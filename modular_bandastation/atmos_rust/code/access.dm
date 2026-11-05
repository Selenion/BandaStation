/// Read access never creates a second authoritative atmosphere in DM.
/turf/proc/get_readonly_air()
	return return_air()

/turf/open/get_readonly_air()
	var/datum/banda_atmos_phase/world_adapter/phase = SSair.native_atmos
	if(!phase || !banda_atmos_id)
		banda_air_requested = TRUE
		return air
	if(!banda_air_present)
		banda_air_requested = TRUE
		return air
	return phase.read_air(src)

/// Mutable access is confined to callbacks between native calculations.
/turf/proc/get_mutable_air()
	return return_air()

/turf/open/get_mutable_air()
	var/datum/banda_atmos_phase/world_adapter/phase = SSair.native_atmos
	if(!phase || !banda_atmos_id)
		banda_air_requested = TRUE
		return air
	if(!phase.in_safe_callback || phase.running)
		CRASH("Mutable turf atmosphere requires a safe callback")
	var/datum/banda_atmos_cell/cell = phase.ensure_binding(src)
	if(!banda_air_present || cell?.pending_rebind)
		banda_air_requested = TRUE
		return cell?.pending_rebind ? cell.read_pending_air() : air
	return phase.borrow_air(cell)

/turf/proc/has_turf_air()
	return FALSE

/turf/open/has_turf_air()
	// Reservation/shuttle code can temporarily block a turf that still owns its mixture.
	return !!air || (!!SSair.native_atmos && banda_atmos_id && banda_air_present && (flags_1 & INITIALIZED_1))

/// Callers requesting a result use the synchronous world boundary; no proc may sleep here.
/datum/banda_atmos_phase/world_adapter/proc/invoke(datum/callback/operation, commit_on_error = FALSE)
	SHOULD_NOT_SLEEP(TRUE)
	if(closed)
		CRASH("Synchronous atmosphere access while native state is unavailable")
	// Returning a fake sample would change breathing and gas conservation. Synchronize on demand.
	enter_safe_phase()
	// Synchronous callers need no queued wrapper datum or boxed return value.
	var/previous_safe = in_safe_callback
	in_safe_callback = TRUE
	try
		. = operation.Invoke()
	catch(var/exception/error)
		// Equipment keeps Banda's completed and partial writes even if a later machine throws.
		finish_callback(previous_safe, !commit_on_error)
		throw error
	finish_callback(previous_safe)

/datum/banda_atmos_cell
	var/native_air_kind = 2
	var/datum/gas_mixture/cached_air
	var/datum/gas_mixture/readonly_air
	var/cache_current = FALSE
	var/cache_cycle = -1
	/// Baseline for direct field/list writes, which bypass gas mutator hooks.
	var/list/committed_air

/// Cache objects are allocated on access, never for every registered coordinate.
/datum/banda_atmos_phase/world_adapter/proc/read_air(turf/open/target)
	var/datum/banda_atmos_cell/cell = ensure_binding(target)
	if(cell.pending_rebind)
		return cell.read_pending_air()
	// Ordinary gas is refreshed by publication; space also has a globally shared last_share.
	if(!cell.cache_current || cell.cache_cycle < 0 || (cell.native_air_kind == 3 && cell.cache_cycle != air_cycle))
		cell.refresh_cache()
	if(in_safe_callback)
		var/datum/gas_mixture/working = working_mixtures[cell]
		if(working)
			cell.copy_readonly(working)
	if(!cell.readonly_air)
		cell.copy_readonly(cell.cached_air)
	return cell.readonly_air

/datum/banda_atmos_cell/proc/refresh_cache(list/input, list/metadata, compact = FALSE, input_offset = 0, metadata_offset = 0)
	if(!input)
		input = read_packet()
	if(!metadata)
		metadata = read_metadata()
	cached_air = banda_atmos_decode_air(input, metadata, native_air_kind, cached_air, compact, input_offset, metadata_offset)
	if(istype(cached_air, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/mixture = cached_air
		mixture.banda_cell = src
		mixture.banda_version = binding_version
	cache_current = TRUE
	remember_air(cached_air)
	if(owner.world_owned)
		var/datum/banda_atmos_phase/world_adapter/phase = owner
		cache_cycle = phase.air_cycle
	if(readonly_air)
		copy_readonly(cached_air)

/datum/banda_atmos_cell/proc/remember_air(datum/gas_mixture/air)
	committed_air = list(air.temperature, air.temperature_archived, air.volume, air.moles.Copy(), air.moles_archive.Copy(), air.reaction_results.Copy())

/// Held mutable aliases may have changed; never let a metadata-only packet hide those writes.
/datum/banda_atmos_cell/proc/can_reuse_air_cache()
	return cache_current && cached_air && native_air_kind != 3 && matches_committed_air(cached_air)

/datum/banda_atmos_cell/proc/refresh_service_cache(list/metadata, metadata_offset)
	if(!can_reuse_air_cache())
		refresh_cache()
		return
	// Publication replaces these snapshots even when their values did not change.
	cached_air.moles_archive = cached_air.moles_archive.Copy()
	cached_air.reaction_results = cached_air.reaction_results.Copy()
	cached_air.last_share = metadata[metadata_offset + 7]
	var/datum/banda_atmos_phase/world_adapter/phase = owner
	cache_cycle = phase.air_cycle
	if(readonly_air)
		copy_readonly(cached_air)

/datum/banda_atmos_cell/proc/matches_committed_air(datum/gas_mixture/air)
	return committed_air && air.temperature && air.temperature == committed_air[1] && air.temperature_archived && air.temperature_archived == committed_air[2] && air.volume == committed_air[3] && \
		banda_atmos_same_gases(air.moles, committed_air[4]) && banda_atmos_same_gases(air.moles_archive, committed_air[5]) && banda_atmos_same_gases(air.reaction_results, committed_air[6])

/// Preserve key order, absent versus zero results, and conservatively retain signed-zero writes.
/proc/banda_atmos_same_gases(list/current, list/previous)
	if(length(current) != length(previous))
		return FALSE
	for(var/index in 1 to length(current))
		var/key = current[index]
		if(key != previous[index] || !current[key] || current[key] != previous[key])
			return FALSE
	return TRUE

/datum/banda_atmos_cell/proc/copy_readonly(datum/gas_mixture/source_air)
	if(istype(source_air, /datum/gas_mixture/immutable/space))
		readonly_air = new /datum/gas_mixture/immutable/space
		readonly_air.last_share = source_air.last_share
		return
	if(!readonly_air)
		if(native_air_kind == 1)
			readonly_air = new /datum/gas_mixture/turf/banda_plain_snapshot(source_air.volume)
		else
			readonly_air = new /datum/gas_mixture/turf(source_air.volume)
	banda_atmos_copy_full(readonly_air, source_air)
	var/datum/gas_mixture/turf/readonly_turf = readonly_air
	readonly_turf.banda_readonly = TRUE

/// A directly assigned ordinary mixture has zero vacuum heat capacity, unlike turf gas.
/datum/gas_mixture/turf/banda_plain_snapshot/heat_capacity()
	return values_dot(moles, GAS_META[META_GAS_SPECIFIC_HEAT])

/datum/gas_mixture/turf/banda_plain_snapshot/heat_capacity_archive()
	return values_dot(moles_archive, GAS_META[META_GAS_SPECIFIC_HEAT])

/turf/open
	var/banda_air_requested = FALSE
	/// Presence survives temporary blocks_air changes; the mixture itself belongs to Rust.
	var/banda_air_present = FALSE

/// Copy the full API state; ordinary copy_from intentionally resets archived gas.
/proc/banda_atmos_copy_full(datum/gas_mixture/destination, datum/gas_mixture/source_air)
	if(destination == source_air)
		return
	destination.moles.Cut()
	for(var/gas, amount in source_air.moles)
		destination.moles[gas] = amount
	destination.moles_archive = source_air.moles_archive.Copy()
	destination.temperature = source_air.temperature
	destination.temperature_archived = source_air.temperature_archived
	destination.volume = source_air.volume
	destination.reaction_results = source_air.reaction_results.Copy()
	destination.last_share = source_air.last_share

/proc/banda_atmos_packet_moles(list/input, input_offset = 0)
	var/list/gases = banda_atmos_gases()
	var/list/moles = list()
	var/index = input_offset + 5
	for(var/gas in 1 to input[input_offset + 4])
		moles[gases[input[index] + 1]] = input[index + 1]
		index += 2
	return moles

/// Compare actual overlay references so floor/plane changes need no additional cache invalidation.
/proc/banda_atmos_update_packet_visuals(turf/open/target, list/input, input_offset = 0)
	var/list/gases = banda_atmos_gases()
	var/list/minimums = GAS_META[META_GAS_MOLES_VISIBLE]
	var/list/overlays = GAS_META[META_GAS_OVERLAY]
	var/list/current = target.atmos_overlay_types
	var/plane_offset = GET_TURF_PLANE_OFFSET(target) + 1
	var/visible_count = 0
	var/index = input_offset + 5
	for(var/entry in 1 to input[input_offset + 4])
		var/gas = gases[input[index++] + 1]
		var/amount = input[index++]
		if(!GAS_IS_VISIBLE(gas, amount, minimums[gas]))
			continue
		visible_count++
		var/list/states = overlays[gas][plane_offset]
		if(visible_count > length(current) || current[visible_count] != states[GAS_VISIBLE_STATE(amount)])
			target.update_visuals(banda_atmos_packet_moles(input, input_offset))
			return
	if(visible_count != length(current))
		target.update_visuals(banda_atmos_packet_moles(input, input_offset))

/turf/proc/atmos_electrolyze(working_power, list/electrolyzer_args = list())
	if(SSair.native_atmos && !SSair.native_atmos.in_safe_callback)
		return SSair.native_atmos.invoke(CALLBACK(src, PROC_REF(atmos_electrolyze), working_power, electrolyzer_args))
	var/datum/gas_mixture/environment = return_air()
	return environment?.electrolyze(working_power, electrolyzer_args)

/// Preserve direct replacement semantics: new volume, class, archives and reaction results.
/turf/open/proc/replace_turf_air(datum/gas_mixture/replacement)
	var/datum/banda_atmos_phase/world_adapter/phase = SSair.native_atmos
	if(!phase || !banda_atmos_id || banda_atmos_cell?.pending_rebind)
		air = replacement
		return
	if(!phase.in_safe_callback)
		return phase.invoke(CALLBACK(src, PROC_REF(replace_turf_air), replacement))
	var/datum/banda_atmos_cell/cell = phase.ensure_binding(src)
	if(istype(cell.cached_air, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/old_air = cell.cached_air
		old_air.banda_cell = null
	phase.working_mixtures -= cell
	phase.native(33, cell.native_id, banda_atmos_full_packet(replacement), istype(replacement, /datum/gas_mixture/immutable/space))
	cell.native_air_kind = phase.native(32, cell.native_id)
	cell.cached_air = replacement
	cell.remember_air(replacement)
	cell.cache_current = TRUE
	cell.cache_cycle = phase.air_cycle
	if(istype(replacement, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/mixture = replacement
		mixture.banda_cell = cell
		mixture.banda_version = cell.binding_version
	cell.readonly_air = null
	banda_air_present = TRUE
