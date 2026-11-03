/// Empty by default: deployments opt in with an explicitly installed, matching DLL.
/datum/config_entry/string/rust_atmos_library

/datum/controller/subsystem/air
	var/datum/banda_atmos_phase/world_adapter/native_atmos

/// Rust owns tile gas. DM consumers borrow only the cells they access between calculations.
/// No DM object/reference crosses the DLL boundary. Passive cells need no binding datum.
/datum/banda_atmos_phase/world_adapter
	world_owned = TRUE
	var/initializing = TRUE
	var/synchronizing = FALSE
	var/air_cycle = 0
	var/list/positions = list()
	var/list/dirty_gas = list()
	var/list/dirty_topology = list()

/datum/banda_atmos_phase/world_adapter/New(library_path)
	..()
	if(!native_ready)
		return
	#ifdef TRACK_MAX_SHARE
	native(34, TRUE)
	#endif
	RegisterSignal(SSdcs, COMSIG_GLOB_ATOM_AFTER_POST_INIT, PROC_REF(atom_initialized))

/datum/banda_atmos_phase/world_adapter/proc/atom_initialized(datum/source, atom/initialized)
	SIGNAL_HANDLER
	if(!initializing && isturf(initialized))
		register_turf(initialized)

/datum/banda_atmos_phase/world_adapter/proc/initialize_world()
	log_world("BANDA_ATMOS_WORLD_REGISTER [world.maxx]x[world.maxy]x[world.maxz]")
	// World jobs carry SSair's actual cycle, including gaps while a template is initializing.
	source_cycle_offset = 0
	var/list/batch = list()
	for(var/turf/target as anything in ALL_TURFS())
		if(!(target.flags_1 & INITIALIZED_1))
			continue
		batch += target
		if(length(batch) == 256)
			register_batch(batch)
			batch.Cut()
			CHECK_TICK
	if(length(batch))
		register_batch(batch)
	log_world("BANDA_ATMOS_WORLD_TOPOLOGY [length(positions)] cells")
	for(var/turf/target as anything in positions)
		dirty_topology[target] = TRUE
	flush_topology()
	var/list/active = list()
	for(var/turf/target as anything in SSair.active_turfs)
		if(target.banda_atmos_id)
			active += target.banda_atmos_id
	var/list/conducting = list()
	for(var/turf/target as anything in SSair.active_super_conductivity)
		if(target.banda_atmos_id)
			conducting += target.banda_atmos_id
	native(18, active, conducting)
	for(var/obj/effect/hotspot/spot as anything in SSair.hotspots.Copy())
		adopt_hotspot(spot)
	native(30)
	initializing = FALSE
	log_world("BANDA_ATMOS_WORLD_READY [length(positions)] cells")
	return TRUE

/datum/banda_atmos_phase/world_adapter/proc/register_batch(list/targets)
	var/list/input = list(length(targets))
	for(var/turf/target as anything in targets)
		var/list/air = banda_atmos_compact_packet(banda_atmos_turf_air(target))
		var/turf/open/open = isopenturf(target) ? target : null
		var/list/environment = banda_atmos_environment(target, open?.excited)
		input += list(target.x, target.y, target.z, banda_atmos_context_bits(target), banda_atmos_freeze_permafrost(target), length(air))
		input += air
		input += length(environment)
		input += environment
	var/list/ids = list()
	native(23, input, ids)
	positions.len = max(length(positions), ids[length(ids)])
	for(var/index in 1 to length(targets))
		var/turf/target = targets[index]
		var/id = ids[index]
		target.banda_atmos_id = id
		positions[id] = target
		handoff_air(target)

/datum/banda_atmos_phase/world_adapter/proc/handoff_air(turf/target)
	if(isopenturf(target))
		var/turf/open/open = target
		// Successful Initialize precedes ChangeTurf's inherited-air restoration and reattachment.
		var/datum/banda_atmos_cell/existing = length(cells) >= open.banda_atmos_id ? cells[open.banda_atmos_id] : null
		if(existing?.pending_rebind)
			return
		open.banda_air_present = !!open.air
		if(!open.air)
			return
		if(open.banda_air_requested)
			var/datum/banda_atmos_cell/cell = ensure_binding(target)
			cell.cached_air = open.air
			cell.remember_air(open.air)
			cell.cache_current = TRUE
			if(istype(cell.cached_air, /datum/gas_mixture/turf))
				var/datum/gas_mixture/turf/mixture = cell.cached_air
				mixture.banda_cell = cell
				mixture.banda_version = cell.binding_version
		open.air = null

/datum/banda_atmos_phase/world_adapter/proc/register_turf(turf/target)
	if(closed || !(target.flags_1 & INITIALIZED_1))
		return
	if(!target.banda_atmos_id)
		var/list/id = list()
		native(28, list(target.x, target.y, target.z), id)
		if(id[1])
			target.banda_atmos_id = id[1]
			positions[id[1]] = target
		else
			register_batch(list(target))
	handoff_air(target)
	mark_dirty(target)
	mark_topology(target)

/datum/banda_atmos_phase/world_adapter/proc/ensure_binding(turf/target)
	if(!target || closed)
		return null
	if(!target.banda_atmos_id)
		register_turf(target)
	if(!target.banda_atmos_id)
		return null
	if(target.banda_atmos_cell)
		return target.banda_atmos_cell
	// During Initialize the post-change callback has not reattached the old handler yet.
	if(length(cells) >= target.banda_atmos_id && cells[target.banda_atmos_id])
		return cells[target.banda_atmos_id]
	var/turf/open/open = isopenturf(target) ? target : null
	return bind(target, !!open?.excited, target.banda_atmos_id)

/datum/banda_atmos_phase/world_adapter/proc/mark_dirty(turf/target)
	if(!target || synchronizing || closed || !target.banda_atmos_id)
		return
	dirty_gas[target] = TRUE
	if(in_safe_callback && isopenturf(target) && target.has_turf_air())
		var/datum/banda_atmos_cell/cell = ensure_binding(target)
		if(cell && !cell.pending_rebind)
			borrow_air(cell)

/datum/banda_atmos_phase/world_adapter/proc/mark_topology(turf/target)
	if(target?.banda_atmos_id && !closed)
		dirty_topology[target] = TRUE

/datum/banda_atmos_phase/world_adapter/proc/topology_rebuilt(turf/target)
	if(initializing || closed)
		return
	mark_topology(target)
	// A removed link is absent from the new cache, but its other end must be updated too.
	for(var/direction in GLOB.cardinals_multiz)
		mark_topology(get_step_multiz(target, direction))

/datum/banda_atmos_phase/world_adapter/proc/flush_topology()
	var/list/input = list(0)
	for(var/turf/target as anything in dirty_topology)
		var/list/neighbors = list()
		for(var/turf/neighbor as anything in target.atmos_adjacent_turfs)
			if(!neighbor.banda_atmos_id)
				register_turf(neighbor)
			if(neighbor.banda_atmos_id)
				neighbors += neighbor.banda_atmos_id
		input[1]++
		input += list(target.banda_atmos_id, length(neighbors))
		input += neighbors
		for(var/direction in GLOB.cardinals)
			var/turf/neighbor = get_step(target, direction)
			input += neighbor?.banda_atmos_id || 0
		if(input[1] == 256)
			native(24, input)
			input = list(0)
			if(initializing)
				CHECK_TICK
	if(input[1])
		native(24, input)
	dirty_topology.Cut()

/// Canonical DM mixtures are imported only where a consumer or scheduling event touched them.
/datum/banda_atmos_phase/world_adapter/proc/flush_inputs()
	var/list/targets = dirty_gas.Copy()
	var/list/schedule = list()
	native(17, schedule)
	var/index = 1
	for(var/section in 1 to 3)
		var/count = schedule[index++]
		for(var/entry in 1 to count)
			var/turf/target = positions[schedule[index++]]
			targets[target] = TRUE
	var/list/input = list(0)
	for(var/turf/target as anything in targets)
		var/list/terrain = banda_atmos_terrain_packet(target)
		input[1]++
		input += list(target.banda_atmos_id, banda_atmos_context_bits(target), banda_atmos_freeze_permafrost(target), 0)
		input += length(terrain)
		input += terrain
		if(input[1] == 256)
			native(25, input)
			input = list(0)
	if(input[1])
		native(25, input)
	dirty_gas.Cut()
	// External code can strengthen an existing hotspot by writing its fields directly.
	for(var/obj/effect/hotspot/spot as anything in SSair.hotspots)
		if(spot.binding?.owner == src && !spot.binding.pending_rebind)
			import_hotspot(spot)

/datum/banda_atmos_phase/world_adapter/run_tick(synchronous = TRUE)
	if(!synchronous || initializing || closed || running || processing || in_safe_callback)
		CRASH("World atmosphere requires an exclusive synchronous phase")
	if(length(uninitialized_cells))
		return FALSE
	// Source effects queued between calculations must be committed before the next frame.
	processing = TRUE
	try
		apply_frame()
	catch(var/exception/error)
		processing = FALSE
		throw error
	processing = FALSE
	if(length(uninitialized_cells))
		return FALSE
	flush_topology()
	flush_inputs()
	running = TRUE
	native(16, SSair.times_fired)
	air_cycle = SSair.times_fired
	running = FALSE
	processing = TRUE
	try
		apply_frame()
	catch(var/exception/publication_error)
		processing = FALSE
		throw publication_error
	processing = FALSE
	return TRUE

/datum/banda_atmos_phase/world_adapter/apply_frame()
	SHOULD_NOT_SLEEP(TRUE)
	while(TRUE)
		project_changes()
		while(length(waiting))
			var/datum/banda_atmos_safe/callback = waiting[1]
			waiting.Cut(1, 2)
			run_callback(callback)
		project_changes()
		var/list/commands = list()
		native(5, commands)
		if(!length(commands))
			break
		var/datum/banda_atmos_safe/world_commands/world/executor = new()
		executor.owner = src
		executor.run_args = list(commands)
		run_callback(executor)
	refresh_debug_overlays()

/datum/banda_atmos_phase/world_adapter/proc/project_changes()
	var/list/ids = list()
	native(26, ids)
	synchronizing = TRUE
	try
		for(var/start = 1; start <= length(ids); start += 256)
			var/list/requests = list()
			for(var/offset = start; offset < min(length(ids) + 1, start + 256); offset++)
				var/id = ids[offset]
				var/datum/banda_atmos_cell/binding = length(cells) >= id ? cells[id] : null
				requests += list(id, !!binding?.cached_air || !!binding?.readonly_air)
			var/list/output = list()
			native(35, requests, output)
			var/index = 1
			while(index <= length(output))
				var/id = output[index++]
				var/version = output[index++]
				var/turf/target = positions[id]
				if(target.x != output[index++] || target.y != output[index++] || target.z != output[index++])
					CRASH("Native atmosphere coordinate registry mismatch")
				var/kind = output[index++]
				var/count = output[index++]
				var/list/air = output.Copy(index, index + count)
				index += count
				count = output[index++]
				var/list/metadata = output.Copy(index, index + count)
				index += count
				var/datum/banda_atmos_cell/cell = ensure_binding(target)
				if(cell.binding_version == version && !cell.pending_rebind)
					cell.native_air_kind = kind
					cell.sync_air(air, metadata, TRUE)
	catch(var/exception/error)
		synchronizing = FALSE
		throw error
	synchronizing = FALSE

/datum/banda_atmos_phase/world_adapter/borrow_air(datum/banda_atmos_cell/cell)
	if(!in_safe_callback || running || !cell || cell.owner != src || cell.pending_rebind)
		CRASH("Mutable world atmosphere access outside a safe phase")
	if(!cell.cache_current || cell.cache_cycle < 0 || (cell.native_air_kind == 3 && cell.cache_cycle != air_cycle))
		cell.refresh_cache()
	working_mixtures[cell] = cell.cached_air
	dirty_gas[cell.target] = TRUE
	return cell.cached_air

/datum/banda_atmos_phase/world_adapter/Destroy()
	UnregisterSignal(SSdcs, COMSIG_GLOB_ATOM_AFTER_POST_INIT)
	QDEL_LIST(debug_groups)
	if(native_id)
		native(10)
		for(var/datum/banda_atmos_cell/cell as anything in cells)
			if(cell?.pending_rebind && !cell.change_depth)
				cell.finish_rebind(TRUE)
	if(SSair.native_atmos == src)
		SSair.native_atmos = null
	if(native_id && length(positions))
		restore_dm_air()
	positions.Cut()
	dirty_gas.Cut()
	dirty_topology.Cut()
	return ..()

/// Closing restores every coordinate, including cells that never had a lazy DM binding.
/datum/banda_atmos_phase/world_adapter/proc/restore_dm_air()
	var/list/ids = list()
	for(var/turf/target as anything in positions)
		ids += target.banda_atmos_id
		if(length(ids) == 256)
			restore_dm_batch(ids)
			ids.Cut()
	if(length(ids))
		restore_dm_batch(ids)
	var/list/schedule = list()
	native(17, schedule)
	var/index = 1
	for(var/queue in list(SSair.active_turfs, null, SSair.active_super_conductivity))
		var/count = schedule[index++]
		for(var/entry in 1 to count)
			var/turf/target = positions[schedule[index++]]
			if(queue && target && !target.banda_atmos_cell?.pending_rebind)
				var/list/destination = queue
				destination |= target
	// Original DM groups stop receiving updates while native atmosphere is enabled.
	for(var/datum/excited_group/group as anything in SSair.excited_groups.Copy())
		group.garbage_collect()
	var/groups = schedule[index++]
	for(var/entry in 1 to groups)
		var/datum/excited_group/group = new()
		var/breakdown = schedule[index++]
		var/dismantle = schedule[index++]
		var/count = schedule[index++]
		for(var/member in 1 to count)
			var/turf/open/target = positions[schedule[index++]]
			if(isopenturf(target) && !target.banda_atmos_cell?.pending_rebind)
				group.add_turf(target)
		group.breakdown_cooldown = breakdown
		group.dismantle_cooldown = dismantle
	// Cell destruction detaches signals/hotspots. It must not wake sleeping tiles on handback.
	for(var/datum/banda_atmos_cell/cell as anything in cells)
		if(!cell)
			continue
		qdel(cell)
	cells.Cut()

/datum/banda_atmos_phase/world_adapter/proc/restore_dm_batch(list/ids)
	var/list/output = list()
	native(27, ids, output)
	var/index = 1
	while(index <= length(output))
		var/turf/target = positions[output[index++]]
		index += 4 // binding version and coordinates
		var/kind = output[index++]
		var/count = output[index++]
		var/list/input = output.Copy(index, index + count)
		index += count
		count = output[index++]
		var/list/metadata = output.Copy(index, index + count)
		index += count
		target.banda_atmos_id = 0
		SSair.active_turfs -= target
		SSair.active_super_conductivity -= target
		if(target.banda_atmos_cell?.pending_rebind)
			continue
		target.temperature = metadata[1]
		target.temperature_archived = metadata[2]
		target.archived_cycle = metadata[3]
		target.current_cycle = metadata[4]
		if(!isopenturf(target))
			continue
		var/turf/open/open = target
		open.banda_air_present = FALSE
		if(kind == 4)
			open.air = null
			continue
		var/datum/gas_mixture/restored = banda_atmos_decode_air(input, metadata, kind)
		var/datum/gas_mixture/cached = target.banda_atmos_cell?.cached_air
		if(kind == 3 && isspaceturf(target))
			var/turf/open/space/space = target
			space.restore_native_space_air(restored.last_share)
		else if(cached)
			banda_atmos_copy_full(cached, restored)
			open.air = cached
		else
			open.air = restored
		open.significant_share_ticker = metadata[5]
		open.excited = metadata[6]
		open.pressure_difference = metadata[8]
		open.pressure_direction = metadata[9]

/turf/open/space/proc/restore_native_space_air(last_share)
	air = space_gas
	air.last_share = last_share

/datum/banda_atmos_safe/world_commands/world/on_run(list/commands)
	var/datum/banda_atmos_phase/world_adapter/world_owner = owner
	for(var/index = 1; index <= length(commands); index += 5)
		var/turf/target = world_owner.positions[commands[index]]
		var/datum/banda_atmos_cell/cell = world_owner.ensure_binding(target)
		if(cell && !cell.pending_rebind && cell.binding_version == commands[index + 1])
			// Held aliases can change their lists directly; unborrowed cells have no such aliases.
			if(cell.cached_air && isopenturf(target) && target.has_turf_air())
				world_owner.borrow_air(cell)
			cell.execute(commands[index + 2], commands[index + 3], commands[index + 4])
