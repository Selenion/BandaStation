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
	var/binding_version = 1
	var/pending_rebind = FALSE
	var/change_depth = 0
	var/inherit_group = TRUE
	var/inherit_gas = TRUE
	var/pending_conduction = FALSE
	var/pending_control
	var/pending_archive_cycle
	var/pending_archive_temperature

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
	if(owner && !QDELETED(owner))
		if(!owner.world_owned)
			owner.native(6, native_id)
		owner.cells[native_id] = null
	owner = null
	target = null
	if(istype(cached_air, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/mixture = cached_air
		mixture.banda_cell = null
	cached_air = null
	committed_air = null
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
	var/datum/gas_mixture/working = owner.working_mixtures[src]
	if(owner.in_safe_callback && working)
		return working.copy()
	if(!input)
		input = read_packet()
	if(!metadata)
		metadata = read_metadata()
	var/turf/open/open = isopenturf(target) && target.has_turf_air() ? target : null
	return banda_atmos_decode_air(input, metadata, owner.world_owned ? native_air_kind : (istype(open?.air, /datum/gas_mixture/immutable/space) ? 3 : 2))

/proc/banda_atmos_decode_air(list/input, list/metadata, kind, datum/gas_mixture/air, compact = FALSE)
	if(air)
		// Keep previously borrowed mixtures and their moles list alive across publication.
		air.moles.Cut()
		// copy_full replaced these two lists; only the live moles list retained its identity.
		air.moles_archive = list()
		air.reaction_results = list()
	else if(kind == 3)
		air = new /datum/gas_mixture/immutable/space
	else if(kind == 1 || kind == 4)
		air = new /datum/gas_mixture(input[3])
	else
		air = new /datum/gas_mixture/turf(input[3])
	air.volume = input[3]
	air.temperature = input[2]
	var/list/gases = banda_atmos_gases()
	var/index = 5
	for(var/gas in 1 to input[4])
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
	air.temperature_archived = metadata[13]
	air.last_share = metadata[7]
	index = 15
	for(var/gas in 1 to metadata[14])
		air.moles_archive[gases[metadata[index] + 1]] = metadata[index + 1]
		index += 2
	return air

/datum/banda_atmos_cell/proc/read_metadata()
	var/list/output = list()
	owner.native(12, native_id, output)
	return output

/datum/banda_atmos_cell/proc/read_packet()
	var/list/input = list()
	owner.native(4, native_id, input)
	return input

/datum/banda_atmos_cell/proc/sync_air(list/input, list/metadata, compact = FALSE)
	if(!target || QDELETED(src) || pending_rebind)
		return
	if(!metadata)
		metadata = read_metadata()
	if(!input)
		input = read_packet()
	target.temperature = metadata[1]
	target.temperature_archived = metadata[2]
	target.archived_cycle = metadata[3] ? metadata[3] + owner.source_cycle_offset : 0
	target.current_cycle = metadata[4] ? metadata[4] + owner.source_cycle_offset : 0
	if(!isopenturf(target) || !target.has_turf_air())
		return
	var/turf/open/open = target
	if(owner.world_owned)
		if(cached_air || readonly_air)
			refresh_cache(input, metadata, compact)
	else
		var/datum/gas_mixture/air = read_air(input, metadata)
		open.air.copy_from(air)
		open.air.reaction_results = air.reaction_results.Copy()
		open.air.moles_archive = air.moles_archive.Copy()
		open.air.temperature_archived = air.temperature_archived
		open.air.last_share = metadata[7]
	open.significant_share_ticker = metadata[5]
	open.excited = metadata[6]
	open.pressure_difference = metadata[8]
	open.pressure_direction = metadata[9]
	open.update_visuals(owner.world_owned ? banda_atmos_packet_moles(input) : null)
	sync_hotspot(input, metadata)

/datum/banda_atmos_cell/proc/sync_hotspot(list/input, list/metadata)
	var/turf/open/open = target
	var/obj/effect/hotspot/spot = open.active_hotspot
	updating_hotspot = TRUE
	if(input[length(input) - 3])
		if(!spot)
			spot = new /obj/effect/hotspot/banda_native(open, input[length(input) - 1], input[length(input) - 2], src)
		spot.binding = src
		spot.temperature = input[length(input) - 2]
		spot.volume = input[length(input) - 1]
		spot.just_spawned = metadata[10]
		spot.bypassing = metadata[11]
		spot.cold_fire = metadata[12]
		spot.update_native_presentation()
	else if(spot)
		qdel(spot)
	updating_hotspot = FALSE

/datum/banda_atmos_cell/proc/expose(temperature, volume, strengthen)
	if(temperature < TCMB)
		CRASH("Native hotspot temperature must be at least TCMB")
	var/datum/banda_atmos_safe/expose/callback = new()
	callback.owner = owner
	callback.run_args = list(src, binding_version, temperature, volume, strengthen)
	owner.waiting += callback
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
			// shortcut: turf.should_atmos_process ignores air; revisit if a gas-dependent turf override is added.
			var/needs_air = second && open && (!owner.world_owned || open._listen_lookup?[COMSIG_TURF_EXPOSE] || open.should_atmos_process(null, first))
			target.temperature_expose(needs_air ? open.return_air() : null, first)
		if(14)
			var/turf/open/open = target
			open.pressure_difference = first
			open.pressure_direction = second
			open.high_pressure_movements()
		else
			CRASH("Unexpected native atmosphere world command [kind]")
