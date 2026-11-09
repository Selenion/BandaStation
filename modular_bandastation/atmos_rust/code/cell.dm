/turf
	/// Created lazily for world-owned cells or explicitly for a small native region.
	var/datum/banda_atmos_cell/banda_atmos_cell
	/// Physical cell identity survives replacement through the coordinate registry.
	var/banda_atmos_id = 0

/datum/banda_atmos_cell
	var/datum/banda_atmos_phase/owner
	var/turf/target
	var/native_id
	/// Null uses a separate BYOND probability roll for every emitted random effect; numbers are test overrides.
	var/rng_bits
	var/updating_hotspot = FALSE
	/// A gameplay constructor ran while this binding could not accept its hotspot yet.
	var/hotspots_pending = FALSE
	var/binding_version = 1
	var/pending_rebind = FALSE
	var/change_depth = 0
	var/inherit_group = TRUE
	var/inherit_gas = TRUE
	var/pending_conduction = FALSE
	var/pending_control
	var/pending_archive_cycle
	var/pending_archive_temperature
	/// Only a live native reference needs the old DM alias to survive destruction callbacks.
	var/datum/gas_mixture/reference_old_air

/datum/banda_atmos_cell/New(datum/banda_atmos_phase/phase, turf/cell, active, registered_id)
	..()
	owner = phase
	target = cell
	var/turf/open/open = isopenturf(cell) && cell.has_turf_air() ? cell : null
	if(registered_id)
		native_id = registered_id
		binding_version = owner.native(22, native_id)
		native_air_kind = owner.native(32, native_id)
	else
		var/datum/gas_mixture/air = open ? open.air : new /datum/gas_mixture(CELL_VOLUME)
		if(!open)
			air.temperature = TCMB
		native_id = owner.native(2, packet(air), context_bits(), freeze_permafrost(), list(cell.x, cell.y, cell.z), environment(active))
	if(!owner.world_owned)
		if(open)
			SSair.remove_from_active(open)
		SSair.active_super_conductivity -= cell
	cell.banda_atmos_cell = src
	RegisterSignal(cell, COMSIG_TURF_CHANGE, PROC_REF(turf_changing))
	RegisterSignal(cell, COMSIG_ATOM_AFTER_SUCCESSFUL_INITIALIZE, PROC_REF(turf_initialized))

/datum/banda_atmos_cell/Destroy()
	reference_old_air = null
	if(target)
		if(isopenturf(target))
			var/turf/open/open = target
			if(open.active_hotspot?.binding == src)
				var/obj/effect/hotspot/spot = open.active_hotspot
				spot.binding = null
				SSair.hotspots |= spot
		UnregisterSignal(target, list(COMSIG_TURF_CHANGE, COMSIG_ATOM_AFTER_SUCCESSFUL_INITIALIZE))
		target.banda_atmos_cell = null
		if(isopenturf(target) && !owner?.world_owned)
			SSair.add_to_active(target)
		if(!owner?.world_owned && (target.flags_1 & INITIALIZED_1))
			target.consider_superconductivity(TRUE)
	owner?.uninitialized_cells.Remove(src)
	if(owner?.effect_mixtures)
		owner.effect_mixtures -= src
	if(owner && !QDELETED(owner))
		if(!owner.world_owned)
			owner.native(6, native_id)
		owner.cells[native_id] = null
	forget_committed_air()
	owner = null
	target = null
	if(istype(cached_air, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/mixture = cached_air
		mixture.banda_cell = null
	cached_air = null
	readonly_air = null
	return ..()

/datum/banda_atmos_cell/proc/control(mode)
	if(updating_hotspot)
		return TRUE
	if(owner.world_owned)
		var/datum/banda_atmos_phase/world_adapter/world_owner = owner
		world_owner.mark_dirty(target)
	if(isopenturf(target) && target.has_turf_air())
		var/turf/open/open = target
		if(mode == 0 || mode == 1)
			open.excited = TRUE
			open.significant_share_ticker = 0
		else if(mode == 2 || mode == 3)
			open.excited = FALSE
	if(pending_rebind)
		if(mode == 1 || mode == 3)
			inherit_group = FALSE
		if(mode <= 3)
			pending_control = mode
		if(mode == 4 || mode == 5)
			pending_conduction = TRUE
		return TRUE
	owner.native(19, native_id, mode)
	return TRUE

/datum/banda_atmos_cell/proc/packet(datum/gas_mixture/air)
	return banda_atmos_air_packet(air)

/datum/banda_atmos_cell/proc/context_bits()
	return banda_atmos_context_bits(target, rng_bits)

/datum/banda_atmos_cell/proc/freeze_permafrost()
	return banda_atmos_freeze_permafrost(target)

/datum/banda_atmos_cell/proc/refresh_context()
	owner.native(7, native_id, context_bits(), freeze_permafrost())
	owner.native(13, native_id, banda_atmos_terrain_packet(target))

/// shortcut: explicit small closed test regions; use cached topology before whole-map integration.
/datum/banda_atmos_cell/proc/refresh_neighbors()
	var/list/neighbors = list()
	for(var/turf/open/neighbor as anything in target.atmos_adjacent_turfs)
		var/datum/banda_atmos_cell/binding = neighbor.banda_atmos_cell
		if(!binding || binding.owner != owner || QDELETED(binding))
			CRASH("Experimental atmosphere region must include every open neighbor")
		neighbors += binding.native_id
	var/list/thermal_neighbors = list()
	for(var/direction in GLOB.cardinals)
		var/turf/neighbor = get_step(target, direction)
		var/datum/banda_atmos_cell/binding = neighbor?.banda_atmos_cell
		if(binding && binding.owner == owner && !QDELETED(binding))
			thermal_neighbors += binding.native_id
		else if(neighbor?.thermal_conductivity)
			CRASH("Experimental atmosphere region must include every conducting neighbor")
		else
			thermal_neighbors += 0
	owner.native(11, native_id, neighbors, thermal_neighbors)

/// Detached completed-frame snapshot. Mutating it cannot change the worker's state.
/datum/banda_atmos_cell/proc/read_air(list/input, list/metadata)
	var/datum/gas_mixture/working = owner.working_mixtures[src] || owner.effect_mixtures?[src]
	if(owner.in_safe_callback && working)
		return working.copy()
	if(!input)
		input = read_packet()
	if(!metadata)
		metadata = read_metadata()
	var/turf/open/open = isopenturf(target) && target.has_turf_air() ? target : null
	return banda_atmos_decode_air(input, metadata, owner.world_owned ? native_air_kind : (istype(open?.air, /datum/gas_mixture/immutable/space) ? 3 : 2))

/proc/banda_atmos_decode_air(list/input, list/metadata, kind, datum/gas_mixture/air, compact = FALSE, input_offset = 0, metadata_offset = 0)
	if(air)
		// Keep previously borrowed mixtures and their moles list alive across publication.
		air.moles.Cut()
		// copy_full replaced these two lists; only the live moles list retained its identity.
		air.moles_archive = list()
		air.reaction_results = list()
	else if(kind == 3)
		air = new /datum/gas_mixture/immutable/space
	else if(kind == 1 || kind == 4)
		air = new /datum/gas_mixture(input[input_offset + 3])
	else
		air = new /datum/gas_mixture/turf(input[input_offset + 3])
	air.volume = input[input_offset + 3]
	air.temperature = input[input_offset + 2]
	var/list/gases = banda_atmos_gases()
	var/index = input_offset + 5
	for(var/gas in 1 to input[input_offset + 4])
		air.moles[gases[input[index] + 1]] = input[index + 1]
		index += 2
	var/list/reactions = banda_atmos_reactions()
	if(compact)
		var/count = input[index++]
		for(var/entry in 1 to count)
			air.reaction_results[reactions[input[index] + 1]] = input[index + 1]
			index += 2
	else
		for(var/reaction in reactions)
			if(input[index])
				air.reaction_results[reaction] = input[index + 1]
			index += 2
	banda_atmos_decode_archive(air, metadata, metadata_offset, gases)
	return air

/proc/banda_atmos_decode_archive(datum/gas_mixture/air, list/metadata, metadata_offset, list/gases)
	air.temperature_archived = metadata[metadata_offset + 13]
	air.last_share = metadata[metadata_offset + 7]
	var/index = metadata_offset + 15
	for(var/gas in 1 to metadata[metadata_offset + 14])
		air.moles_archive[gases[metadata[index] + 1]] = metadata[index + 1]
		index += 2

/datum/banda_atmos_cell/proc/read_metadata()
	var/list/output = list()
	owner.native(12, native_id, output)
	return output

/datum/banda_atmos_cell/proc/read_packet()
	var/list/input = list()
	owner.native(4, native_id, input)
	return input

/datum/banda_atmos_cell/proc/sync_air(list/input, list/metadata, compact = FALSE, input_offset = 0, metadata_offset = 0, input_count = 0, metadata_count = 0, visuals_only = FALSE, list/thermal_baseline)
	if(!target || QDELETED(src) || pending_rebind)
		return
	if(!metadata)
		metadata = read_metadata()
	if(!input)
		input = read_packet()
	target.temperature = metadata[metadata_offset + 1]
	target.temperature_archived = metadata[metadata_offset + 2]
	target.archived_cycle = metadata[metadata_offset + 3] ? metadata[metadata_offset + 3] + owner.source_cycle_offset : 0
	target.current_cycle = metadata[metadata_offset + 4] ? metadata[metadata_offset + 4] + owner.source_cycle_offset : 0
	if(!isopenturf(target) || !target.has_turf_air())
		return
	var/turf/open/open = target
	if(owner.world_owned)
		if(cached_air || readonly_air)
			if(thermal_baseline)
				refresh_thermal_cache(input[input_offset + 2], metadata, metadata_offset, metadata_count > 12, thermal_baseline)
			else if(compact && metadata_count == 12)
				refresh_service_cache(metadata, metadata_offset)
			else
				refresh_cache(input, metadata, compact, input_offset, metadata_offset)
	else
		var/datum/gas_mixture/air = read_air(input, metadata)
		open.air.copy_from(air)
		open.air.reaction_results = air.reaction_results.Copy()
		open.air.moles_archive = air.moles_archive.Copy()
		open.air.temperature_archived = air.temperature_archived
		open.air.last_share = metadata[metadata_offset + 7]
	open.significant_share_ticker = metadata[metadata_offset + 5]
	open.excited = metadata[metadata_offset + 6]
	open.pressure_difference = metadata[metadata_offset + 8]
	open.pressure_direction = metadata[metadata_offset + 9]
	sync_presentation(input, metadata, input_offset, metadata_offset, input_count, visuals_only)

/// Numeric publication has already completed; callbacks retain the original per-turf order.
/datum/banda_atmos_cell/proc/sync_presentation(list/input, list/metadata, input_offset, metadata_offset, input_count, visuals_only)
	var/turf/open/open = target
	if(visuals_only && !input_count)
		// Empty presentation still clears old overlays and retires or adopts an existing hotspot.
		if(length(open.atmos_overlay_types))
			open.apply_gas_overlays(list())
		if(open.active_hotspot || hotspots_pending)
			sync_hotspot(input, metadata, absent = TRUE)
		return
	if(owner.world_owned)
		if(visuals_only)
			banda_atmos_update_visual_states(open, input, input_offset)
		else
			banda_atmos_update_packet_visuals(open, input, input_offset)
	else
		open.update_visuals()
	var/hotspot_end = input_count ? input_offset + input_count : length(input)
	if(input[hotspot_end - 3] || open.active_hotspot || hotspots_pending)
		sync_hotspot(input, metadata, input_offset, metadata_offset, input_count)

/datum/banda_atmos_cell/proc/sync_hotspot(list/input, list/metadata, input_offset = 0, metadata_offset = 0, input_count = 0, absent = FALSE)
	var/turf/open/open = target
	var/obj/effect/hotspot/spot = open.active_hotspot
	if(!input_count)
		input_count = length(input) - input_offset
	var/end = input_offset + input_count
	updating_hotspot = TRUE
	if(!absent && input[end - 3])
		if(!spot)
			spot = new /obj/effect/hotspot/banda_native(open, input[end - 1], input[end - 2], src)
		spot.binding = src
		spot.temperature = input[end - 2]
		spot.volume = input[end - 1]
		spot.just_spawned = metadata[metadata_offset + 10]
		spot.bypassing = metadata[metadata_offset + 11]
		spot.cold_fire = metadata[metadata_offset + 12]
		spot.update_native_presentation()
	else if(spot)
		qdel(spot)
	updating_hotspot = FALSE
	if(hotspots_pending)
		adopt_pending_hotspots()

/datum/banda_atmos_cell/proc/expose(temperature, volume, strengthen)
	if(temperature < TCMB)
		CRASH("Native hotspot temperature must be at least TCMB")
	var/datum/banda_atmos_safe/expose/callback = new()
	callback.owner = owner
	callback.run_args = list(src, binding_version, temperature, volume, strengthen)
	owner.waiting += callback
	if(owner.world_owned)
		var/datum/banda_atmos_phase/world_adapter/phase = owner
		if(phase.effects_ready)
			return
	if(!owner.running && !owner.processing && !owner.in_safe_callback)
		owner.processing = TRUE
		try
			owner.apply_frame()
		catch(var/exception/error)
			owner.processing = FALSE
			throw error
		owner.processing = FALSE

/// Safe borrowers mutate both current gas and its archives/results; copy_air has narrower semantics.
/datum/banda_atmos_cell/proc/commit_packet(datum/gas_mixture/air)
	return banda_atmos_full_packet(air)

/datum/banda_atmos_cell/proc/commit_air(datum/gas_mixture/air, synchronize = TRUE, wake = TRUE)
	if(owner.world_owned)
		if(matches_committed_air(air))
			return FALSE
		owner.native(36, native_id, !!wake, banda_atmos_compact_packet(air, TRUE))
		remember_air(air)
	else
		owner.native(3, native_id, wake ? 2 : 3, 1, commit_packet(air))
	if(synchronize)
		sync_air()
	return TRUE

/datum/banda_atmos_cell/proc/write_air(datum/gas_mixture/air, release = FALSE)
	if(pending_rebind)
		if(release && isopenturf(target) && target.has_turf_air())
			var/turf/open/open = target
			open.air.merge(air)
			owner.native(3, native_id, 1, 1, packet(air))
			return TRUE
		return FALSE
	if(owner.in_safe_callback)
		var/datum/gas_mixture/working = owner.borrow_air(src)
		if(release)
			working.merge(air)
		else
			working.copy_from(air)
		return TRUE
	owner.native(3, native_id, release ? 1 : 0, 1, packet(air))
	if(!owner.running)
		sync_air()
	return TRUE

/// Called after publication; object feedback is visible to the NEXT native calculation.
/datum/banda_atmos_cell/proc/execute(kind, first, second)
	switch(kind)
		if(1)
			var/turf/open/open = target
			open.freeze_turf()
		if(2)
			var/turf/open/open = target
			open.water_vapor_gas_act()
		if(4)
			radiation_pulse(target, max_range = first, threshold = second)
		if(5)
			new /obj/item/stack/sheet/hot_ice(target)
		if(6)
			do_foam(amount = first, holder = target, location = target, foam_type = /datum/effect_system/fluid_spread/foam/metal/resin/halon)
		if(7)
			target.fire_nuclear_particle()
		if(8)
			visible_hallucination_pulse(target, first, second)
		if(9)
			for(var/atom/affected in target)
				if(!QDELETED(affected))
					affected.fire_act(first, second)
		if(10)
			var/turf/open/open = target
			open.burn_tile()
		if(11)
			if(prob(10))
				radiation_pulse(target, max_range = first, threshold = second)
		if(12)
			if(prob(2))
				new /obj/item/stack/sheet/hot_ice(target)
		if(13)
			var/turf/open/open = isopenturf(target) ? target : null
			var/datum/listeners = open?._listen_lookup?[COMSIG_TURF_EXPOSE]
			// A different listener may read or modify the original working mixture.
			if(second && owner.world_owned && istype(listeners, /obj/machinery/atmospherics/components/unary/vent_scrubber) && \
				listeners._signal_procs?[open]?[COMSIG_TURF_EXPOSE] == TYPE_PROC_REF(/atom, check_atmos_process) && !open.should_atmos_process(null, first))
				var/datum/banda_atmos_phase/world_adapter/phase = owner
				var/keys = phase.scrubber_air_keys(open)
				if(!isnull(keys))
					open.temperature_expose(null, first, keys)
					return
			if(second && owner.world_owned && istype(listeners, /obj/machinery/door/firedoor) && \
				listeners._signal_procs?[open]?[COMSIG_TURF_EXPOSE] == TYPE_PROC_REF(/obj/machinery/door/firedoor, process_results) && !open.should_atmos_process(null, first))
				var/datum/banda_atmos_phase/world_adapter/phase = owner
				if(phase.cold_air_version(open))
					// process_results reads gas independently and ignores the signal's mixture argument.
					open.temperature_expose(null, first)
					return
			if(second && owner.world_owned && listeners && !cached_air && !readonly_air && !pending_rebind && banda_atmos_temperature_only_exposure(open, listeners))
				open.temperature_expose(null, first, native_reading = TRUE)
				return
			// shortcut: turf.should_atmos_process ignores air; revisit if a gas-dependent turf override is added.
			var/needs_air = second && open && (!owner.world_owned || listeners || open.should_atmos_process(null, first))
			target.temperature_expose(needs_air ? open.return_air() : null, first)
		if(14)
			var/turf/open/open = target
			open.pressure_difference = first
			open.pressure_direction = second
			open.high_pressure_movements()
		else
			CRASH("Unexpected native atmosphere world command [kind]")

/// Keep the mixture for the entire signal if even one listener may read or retain it.
/proc/banda_atmos_temperature_only_exposure(turf/open/target, listeners)
	if(!islist(listeners))
		return banda_atmos_temperature_only_listener(target, listeners)
	for(var/datum/listener as anything in listeners)
		if(!banda_atmos_temperature_only_listener(target, listener))
			return FALSE
	return TRUE

/proc/banda_atmos_temperature_only_listener(turf/open/target, datum/listener)
	var/callback = listener._signal_procs?[target]?[COMSIG_TURF_EXPOSE]
	// This handler reads its own numeric snapshot; mixed unknown listeners still receive the live mixture.
	if(callback == TYPE_PROC_REF(/obj/machinery/airalarm, check_danger) && istype(listener, /obj/machinery/airalarm))
		return SSair.native_atmos?.inspection_queries_ready
	if(callback == TYPE_PROC_REF(/obj/structure/plasticflaps, check_melt))
		return istype(listener, /obj/structure/plasticflaps)
	if(callback != TYPE_PROC_REF(/atom, check_atmos_process))
		return FALSE
	// These types use only the temperature to enter SSair's exposure queue. Actual exposure can still borrow air.
	var/static/list/temperature_checks = typecacheof(list(/obj/machinery/firealarm, /obj/machinery/light, /obj/machinery/power/apc, /obj/machinery/door/window, /obj/structure/grille, /obj/structure/window))
	return temperature_checks[listener.type]
