/// Empty by default: deployments opt in with an explicitly installed, matching DLL.
/datum/config_entry/string/rust_atmos_library

/// Opt in separately: ordinary native atmosphere keeps its synchronous phase.
/datum/config_entry/flag/rust_atmos_threaded

/datum/controller/subsystem/air
	var/datum/banda_atmos_phase/world_adapter/native_atmos
	/// Worker exposure is drained by an ordinary MC phase before the next calculation.
	var/native_atoms_pending = FALSE

/// Rust owns tile gas. DM consumers borrow only the cells they access between calculations.
/// No DM object/reference crosses the DLL boundary. Passive cells need no binding datum.
/datum/banda_atmos_phase/world_adapter
	world_owned = TRUE
	var/initializing = TRUE
	var/synchronizing = FALSE
	var/air_cycle = 0
	var/threaded = FALSE
	var/worker_cycle = 0
	var/completed_ticks = 0
	var/publication_pending = FALSE
	var/tick_started = 0
	/// Wall latency, including await; TICK_USAGE_REAL cannot span a suspended proc.
	var/last_tick_ms = 0
	var/list/positions = list()
	var/list/dirty_gas = list()
	var/list/dirty_topology = list()
	/// Flat environment and empty presentation records require the newer DLL.
	var/compact_exchange = FALSE
	/// The DLL can send archive and temperature separately from live moles and reaction results.
	var/thermal_publication_ready = FALSE

/datum/banda_atmos_phase/world_adapter/New(library_path)
	..()
	if(!native_ready)
		return
	try
		native_baselines_ready = native(74) == 1
	catch
		native_baselines_ready = FALSE
	try
		// Probe the additive publication operation before registering any world state.
		native(40, list(), 1)
	catch(var/exception/publication_error)
		native_ready = FALSE
		startup_error = "Native publication requires an updated DLL: [publication_error.name]"
		return
	try
		compact_exchange = native(63) == 1
	catch
		compact_exchange = FALSE
	try
		properties_ready = native(50) == 1
	catch
		properties_ready = FALSE
	if(properties_ready)
		try
			configure_visual_publication()
		catch
			visual_publication_ready = FALSE
	if(compact_exchange && visual_publication_ready)
		try
			thermal_publication_ready = native(40, list(), 2, list()) == 1
		catch
			thermal_publication_ready = FALSE
	if(visual_publication_ready && pipe_direct_ready)
		try
			vent_transfer_ready = native(54) == 1
		catch
			vent_transfer_ready = FALSE
	if(pipe_direct_ready)
		try
			scrubber_transfer_ready = native(56) == 1
		catch
			scrubber_transfer_ready = FALSE
	if(scrubber_transfer_ready && visual_publication_ready)
		try
			scrubber_tile_ready = native(58, 1) == 1
		catch
			scrubber_tile_ready = FALSE
	threaded = CONFIG_GET(flag/rust_atmos_threaded)
	try
		operations_ready = native(82) == 1
	catch
		operations_ready = FALSE
	pipelined = CONFIG_GET(flag/rust_atmos_pipeline)
	if(pipelined && (!threaded || !operations_ready))
		native_ready = FALSE
		startup_error = "Pipeline atmosphere requires the worker and gas-operation protocol"
		return
	if(threaded)
		try
			native(38, 0, 0)
		catch(var/exception/error)
			native_ready = FALSE
			startup_error = "Threaded atmosphere requires an updated DLL: [error.name]"
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
		if(!open.air)
			return
		// After the first handoff null means Rust owns the mixture, not that air disappeared.
		open.banda_air_present = TRUE
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
		// New coordinates cannot be inserted into a worker's unfinished graph.
		enter_safe_phase()
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
			if(operation_cells[cell] == TRUE && !cell.cached_air && !cell.readonly_air)
				return
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
	native(compact_exchange ? 63 : 17, schedule)
	var/index = 1
	for(var/section in 1 to 3)
		var/count = schedule[index++]
		for(var/entry in 1 to count)
			var/turf/target = positions[schedule[index++]]
			targets[target] = TRUE
	var/list/input = list(0)
	var/input_operation = compact_exchange ? 64 : 25
	for(var/turf/target as anything in targets)
		input[1]++
		var/context = banda_atmos_context_bits(target)
		var/freeze = banda_atmos_freeze_permafrost(target)
		if(compact_exchange)
			// Keep reading live DM fields; some gameplay code changes them without a signal.
			var/flags = context | (freeze ? 1024 : 0) | (target.blocks_air ? 2048 : 0)
			input += list(target.banda_atmos_id, flags, target.temperature, target.temperature_archived || 0, target.heat_capacity, target.thermal_conductivity, target.atmos_supeconductivity)
		else
			var/list/terrain = banda_atmos_terrain_packet(target)
			input += list(target.banda_atmos_id, context, freeze, 0, length(terrain))
			input += terrain
		if(input[1] == 256)
			native(input_operation, input)
			input = list(0)
	if(input[1])
		native(input_operation, input)
	dirty_gas.Cut()
	// External code can strengthen an existing hotspot by writing its fields directly.
	for(var/obj/effect/hotspot/spot as anything in SSair.hotspots)
		if(spot.binding?.owner == src && !spot.binding.pending_rebind)
			import_hotspot(spot)

/datum/banda_atmos_phase/world_adapter/run_tick(synchronous)
	if(isnull(synchronous))
		synchronous = !threaded
	if((!synchronous && !threaded) || initializing || closed || running || worker_cycle || processing || in_safe_callback)
		CRASH("World atmosphere phase cannot overlap itself")
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
	tick_started = REALTIMEOFDAY
	running = TRUE
	var/cycle = SSair.times_fired
	if(synchronous)
		try
			native(16, cycle)
		catch(var/exception/calculation_error)
			running = FALSE
			throw calculation_error
		running = FALSE
		air_cycle = cycle
		publication_pending = TRUE
	else
		worker_cycle = cycle
		var/success = call_ext(library, "byond,await:banda_atmos_world_tick")(native_id, cycle)
		if(QDELETED(src) || closed)
			return FALSE
		if(!success)
			running = FALSE
			worker_cycle = 0
			native(10)
			CRASH("Native world atmosphere calculation failed")
	try
		if(!synchronous)
			synchronize_calculation()
		finish_publication()
	catch(var/exception/publication_error)
		// A DM effect can throw after Rust has published. Keep its gas, but let the next MC fire run.
		if(!running)
			worker_cycle = 0
		throw publication_error
	worker_cycle = 0
	completed_ticks++
	return TRUE

/// A result-returning API may need fresh gas before SSair resumes. Only wait for numeric work:
/// joining the thread would deadlock its Byond_Return notification on the calling DM thread.
/datum/banda_atmos_phase/world_adapter/proc/synchronize_calculation()
	SHOULD_NOT_SLEEP(TRUE)
	if(!running)
		return
	if(!threaded || !worker_cycle)
		CRASH("Cannot synchronize this atmosphere phase")
	native(38, 1, worker_cycle)
	running = FALSE
	air_cycle = worker_cycle
	publication_pending = TRUE
	// Like Paradise, safe callbacks precede world commands; a breath can precede pressure movement.
	project_changes()

/datum/banda_atmos_phase/world_adapter/proc/finish_publication()
	SHOULD_NOT_SLEEP(TRUE)
	if(!publication_pending)
		return
	processing = TRUE
	try
		apply_frame()
	catch(var/exception/publication_error)
		processing = FALSE
		throw publication_error
	processing = FALSE
	publication_pending = FALSE
	last_tick_ms = (REALTIMEOFDAY - tick_started) * 100

/// Earlier queued work precedes a later synchronous request for a sample or floor replacement.
/datum/banda_atmos_phase/world_adapter/proc/enter_safe_phase()
	SHOULD_NOT_SLEEP(TRUE)
	if(!threaded)
		return
	synchronize_calculation()
	if(processing || in_safe_callback || !length(waiting))
		return
	processing = TRUE
	try
		run_waiting_callbacks()
		project_changes()
	catch(var/exception/error)
		processing = FALSE
		throw error
	processing = FALSE

/datum/banda_atmos_phase/world_adapter/proc/run_waiting_callbacks()
	SHOULD_NOT_SLEEP(TRUE)
	while(length(waiting))
		var/datum/banda_atmos_safe/callback = waiting[1]
		waiting.Cut(1, 2)
		run_callback(callback)

/datum/banda_atmos_phase/world_adapter/apply_frame()
	SHOULD_NOT_SLEEP(TRUE)
	if(pipelined)
		collect_pipeline_commands()
		return
	while(TRUE)
		project_changes()
		run_waiting_callbacks()
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
	clear_air_readings()
	var/list/ids = list()
	var/changes_received = FALSE
	synchronizing = TRUE
	var/thermal_publication = thermal_publication_ready && visual_publication_ready && compact_exchange
	var/publication_operation = thermal_publication ? 73 : (visual_publication_ready ? (compact_exchange ? 65 : 53) : 35)
	var/direct_publication = native_baselines_ready && thermal_publication
	var/direct_batch
	var/list/direct_context
	if(direct_publication)
		direct_context = list(positions, banda_atmos_gases(), banda_atmos_reactions(), GAS_META[META_GAS_OVERLAY], SSmapping, INITIALIZED_1)
	try
		var/list/thermal_ids
		if(thermal_publication)
			thermal_ids = list()
			native(40, ids, 2, thermal_ids)
		else
			native(40, ids, 1) // Sign marks metadata-only changes, absolute value is the cell ID.
		changes_received = TRUE
		var/thermal_index = 1
		var/thermal_count = length(thermal_ids)
		var/next_thermal = thermal_count ? thermal_ids[1] : 0
		var/next_thermal_id = abs(next_thermal)
		for(var/start = 1; start <= length(ids); start += 256)
			var/list/requests = list()
			var/list/thermal_baselines
			var/end = min(length(ids) + 1, start + 256)
			for(var/offset = start; offset < end; offset++)
				var/id = abs(ids[offset])
				var/datum/banda_atmos_cell/binding = length(cells) >= id ? cells[id] : null
				var/cache_mode = !!binding?.cached_air || !!binding?.readonly_air
				if(id == next_thermal_id)
					if(binding?.cached_air && binding.cache_current && binding.committed_from_native && binding.native_air_kind != 3)
						cache_mode = next_thermal < 0 ? 3 : 2
						if(!direct_publication)
							if(!thermal_baselines)
								thermal_baselines = new/list(end - start)
							// Keep the baseline from packet creation even if an earlier callback replaces it.
							thermal_baselines[offset - start + 1] = binding.committed_snapshot()
					thermal_index++
					next_thermal = thermal_index <= thermal_count ? thermal_ids[thermal_index] : 0
					next_thermal_id = abs(next_thermal)
				else if(cache_mode && ids[offset] < 0 && binding.can_reuse_air_cache())
					cache_mode = FALSE
				requests += list(id, cache_mode)
				if(direct_publication)
					requests += (ids[offset] < 0)
			var/list/output = list()
			if(direct_publication)
				direct_batch = native(75, requests)
				while(TRUE)
					var/status = native(76, direct_batch, src, direct_context, output)
					if(status == 1)
						break
					publication_values += length(output)
					if(status == 2)
						ensure_binding(positions[output[1]])
						continue
					var/datum/banda_atmos_cell/cell = positions[output[1]]:banda_atmos_cell
					if(!cell || cell.pending_rebind || cell.binding_version != output[2])
						continue
					var/air_count = output[8]
					var/metadata_offset = 9 + air_count
					if(status == 3)
						cell.sync_presentation(output, output, 8, metadata_offset, air_count, output[7])
					else if(status == 5)
						cell.sync_hotspot(output, output, 8, metadata_offset, air_count)
					else if(status == 4)
						cell.sync_air(output, output, TRUE, 8, metadata_offset, air_count, output[metadata_offset], output[7])
						if(output[7])
							visual_publications++
				direct_batch = null
				continue
			native(publication_operation, requests, output)
			publication_values += length(output)
			var/index = 1
			var/record = 0
			while(index <= length(output))
				record++
				var/id = output[index++]
				var/version = output[index++]
				var/turf/target = positions[id]
				if(target.x != output[index++] || target.y != output[index++] || target.z != output[index++])
					CRASH("Native atmosphere coordinate registry mismatch")
				var/kind = output[index++]
				var/visuals_only = visual_publication_ready ? output[index++] : FALSE
				var/count = output[index++]
				var/air_offset = index - 1
				var/air_count = count
				index += count
				count = output[index++]
				var/metadata_offset = index - 1
				index += count
				var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
				if(!cell)
					cell = ensure_binding(target)
				if(cell.binding_version == version && !cell.pending_rebind)
					cell.native_air_kind = kind
					// Check the live DM fields: earlier callbacks can edit them after Rust took its snapshot.
					if(ids[start + record - 1] < 0 && visuals_only && !air_count && count == 12 && !cell.cached_air && !cell.readonly_air && isopenturf(target))
						var/turf/open/open = target
						var/current_cycle = output[metadata_offset + 4] ? output[metadata_offset + 4] + source_cycle_offset : 0
						var/archived_cycle = output[metadata_offset + 3] ? output[metadata_offset + 3] + source_cycle_offset : 0
						if(open.current_cycle == current_cycle && open.archived_cycle == archived_cycle && open.has_turf_air() && \
							!length(open.atmos_overlay_types) && !open.active_hotspot && !cell.hotspots_pending && \
							open.temperature == output[metadata_offset + 1] && open.temperature_archived == output[metadata_offset + 2] && \
							open.significant_share_ticker == output[metadata_offset + 5] && open.excited == output[metadata_offset + 6] && \
							open.pressure_difference == output[metadata_offset + 8] && open.pressure_direction == output[metadata_offset + 9])
							service_publications_skipped++
							visual_publications++
							continue
					var/list/baseline = visuals_only == 2 ? thermal_baselines[record] : null
					cell.sync_air(output, output, TRUE, air_offset, metadata_offset, air_count, count, visuals_only, baseline)
					if(visuals_only)
						visual_publications++
	catch(var/exception/error)
		if(direct_batch && native_id)
			native(77, direct_batch)
		// Rust already consumed these changes; an aborted projection may leave any cache behind.
		if(!changes_received)
			for(var/datum/banda_atmos_cell/unknown_cache in cells)
				if(!QDELETED(unknown_cache))
					unknown_cache.cache_current = FALSE
		for(var/changed_id in ids)
			var/cache_id = abs(changed_id)
			if(cache_id >= 1 && length(cells) >= cache_id)
				var/datum/banda_atmos_cell/invalidated = cells[cache_id]
				if(invalidated && !QDELETED(invalidated))
					invalidated.cache_current = FALSE
		synchronizing = FALSE
		throw error
	synchronizing = FALSE

/datum/banda_atmos_phase/world_adapter/borrow_air(datum/banda_atmos_cell/cell)
	if(!in_safe_callback || running || !cell || cell.owner != src || cell.pending_rebind)
		CRASH("Mutable world atmosphere access outside a safe phase")
	materialize_operation_cell(cell)
	if(scrubber_tile_pending && !cell.cached_air)
		materialize_scrubber_tile(cell)
	if(!cell.cache_current || cell.cache_cycle < 0 || (cell.native_air_kind == 3 && cell.cache_cycle != air_cycle))
		cell.refresh_cache()
	working_mixtures[cell] = cell.cached_air
	dirty_gas[cell.target] = TRUE
	return cell.cached_air

/datum/banda_atmos_phase/world_adapter/Destroy()
	// Restore the completed prefix before handing air back to DM, even inside a failing callback.
	if(in_safe_callback && !closed && !running)
		finish_callback(FALSE)
	closed = TRUE
	deferred_commands.Cut()
	active_commands.Cut()
	clear_air_readings()
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

/datum/banda_atmos_safe/world_commands/world/on_run(list/commands, start = 1)
	var/datum/banda_atmos_phase/world_adapter/world_owner = owner
	for(var/index = start; index <= length(commands); index += 5)
		if(QDELETED(world_owner) || world_owner.closed)
			return
		var/turf/target = world_owner.positions[commands[index]]
		var/datum/banda_atmos_cell/cell = world_owner.ensure_binding(target)
		if(cell && !cell.pending_rebind && cell.binding_version == commands[index + 1])
			// Borrow before skipping empty notifications so direct edits through held mixtures still commit.
			if(cell.cached_air && isopenturf(target) && target.has_turf_air())
				world_owner.borrow_air(cell)
			if(commands[index + 2] == 13 && isopenturf(target) && !target._listen_lookup?[COMSIG_TURF_EXPOSE] && \
				!(target.flags_1 & ATMOS_IS_PROCESSING_1) && !target.should_atmos_process(null, commands[index + 3]))
				continue
			cell.execute(commands[index + 2], commands[index + 3], commands[index + 4])
