/// Experimental owner of explicitly bound cells; not automatically attached to SSair.
/// Phase separation follows Paradise MILLA (ParadiseSS13/Paradise, AGPL-3.0).
/datum/banda_atmos_phase
	var/native_id
	var/native_ready = FALSE
	var/startup_error
	var/library
	var/running = FALSE
	var/processing = FALSE
	var/closed = FALSE
	var/in_safe_callback = FALSE
	var/world_owned = FALSE
	/// Optional main-thread native snapshots preserve direct edits through issued mixtures.
	var/native_baselines_ready = FALSE
	/// Rows written through the main-thread C API, without a DM numeric packet.
	var/direct_publications = 0
	var/source_cycle_offset = 0
	var/list/cells = list()
	var/list/waiting = list()
	var/list/working_mixtures = list()
	var/list/uninitialized_cells = list()

/datum/banda_atmos_phase/New(library_path)
	..()
	try
		if(!istext(library_path) || !length(library_path))
			throw EXCEPTION("Native atmosphere phase requires an explicit library path")
		if(world.byond_version < 516 || (world.byond_version == 516 && world.byond_build < 1674))
			throw EXCEPTION("Native atmosphere phase requires BYOND 516.1674 or later")
		library = library_path
		native_id = call_ext(library, "byond:banda_atmos")(0)
		if(!native_id)
			throw EXCEPTION("Unable to create native atmosphere phase")
		var/list/abi = list()
		native(9, abi)
		if(length(abi) != 4 || abi[1] != 11 || abi[2] != length(banda_atmos_gases()) || abi[3] != length(banda_atmos_reactions()) || abi[4] != BANDA_ATMOS_CATALOG_SIGNATURE)
			throw EXCEPTION("Atmosphere DLL protocol/catalog does not match the DM build")
		native_ready = TRUE
		try
			pipes_ready = native(41) == 1
		catch
			pipes_ready = FALSE
		if(pipes_ready)
			try
				pipe_direct_ready = native(49) == 1
			catch
				pipe_direct_ready = FALSE
		if(pipe_direct_ready)
			try
				pipe_owned_ready = native(68) == 1
			catch
				pipe_owned_ready = FALSE
	catch(var/exception/error)
		startup_error = error.name

/datum/banda_atmos_phase/Destroy()
	closed = TRUE
	operation_cells.Cut()
	pipes_ready = FALSE
	pipe_direct_ready = FALSE
	while(length(pipe_bindings))
		qdel(pipe_bindings[length(pipe_bindings)])
	if(native_id)
		// Preserve external writes when handing cells back to the ordinary DM simulation.
		native(10)
	for(var/datum/banda_atmos_cell/cell as anything in cells)
		if(!cell)
			continue
		if(cell.pending_rebind && !cell.change_depth)
			cell.finish_rebind(TRUE)
		cell.sync_air()
		cell.owner = null
		qdel(cell)
	cells.Cut()
	working_mixtures.Cut()
	uninitialized_cells.Cut()
	QDEL_LIST(waiting)
	if(native_id)
		native(1)
		native_id = null
	return ..()

/// All synchronous DLL calls copy numeric packets before returning.
/datum/banda_atmos_phase/proc/native(operation, ...)
	if(world_owned)
		var/datum/banda_atmos_phase/world_adapter/world_owner = src
		switch(operation)
			if(3, 6, 20, 21, 33, 36)
				world_owner.invalidate_air_reading(args[2])
			if(1, 10, 16, 25, 30, 38, 64)
				world_owner.clear_air_readings()
	// BYOND 516 retains list arguments passed to call_ext through arglist after the call.
	// Explicit arities let temporary packets be released without clearing callers' lists.
	var/result
	switch(length(args))
		if(0, 1)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id)
		if(2)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2])
		if(3)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2], args[3])
		if(4)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2], args[3], args[4])
		if(5)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2], args[3], args[4], args[5])
		if(6)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2], args[3], args[4], args[5], args[6])
		if(7)
			result = call_ext(library, "byond:banda_atmos")(operation, native_id, args[2], args[3], args[4], args[5], args[6], args[7])
		else
			throw EXCEPTION("Native atmosphere operation [operation] failed: Invalid arguments")
	if(!result)
		var/error = call_ext(library, "banda_atmos_error")()
		throw EXCEPTION("Native atmosphere operation [operation] failed: [error]")
	return result

/datum/banda_atmos_phase/proc/bind(turf/target, active = TRUE, registered_id)
	if(!native_ready)
		CRASH("Native atmosphere is unavailable: [startup_error]")
	if(!target || closed || (running && (!world_owned || !registered_id)) || (processing && !world_owned) || target.banda_atmos_cell)
		CRASH("Cannot bind this turf to the experimental atmosphere phase")
	if(isopenturf(target) && target.has_turf_air())
		var/turf/open/open = target
		if(!open.air && !registered_id)
			CRASH("Open native turf requires a gas mixture")
		if(istype(open.air, /datum/gas_mixture/immutable) && !istype(open.air, /datum/gas_mixture/immutable/space))
			CRASH("Unsupported immutable turf atmosphere")
		if(open.active_hotspot && !world_owned)
			CRASH("Existing hotspot import requires its native presentation binding")
	else
		active = FALSE
	var/datum/banda_atmos_cell/cell = new(src, target, active, registered_id)
	cells.len = max(cells.len, cell.native_id)
	cells[cell.native_id] = cell
	return cell

/// BYOND await yields this caller; the native worker performs no game callbacks.
/datum/banda_atmos_phase/proc/run_tick(synchronous = FALSE)
	if(closed || running || processing || in_safe_callback)
		CRASH("Atmosphere phase cannot overlap itself")
	if(length(uninitialized_cells))
		return FALSE
	for(var/datum/banda_atmos_cell/cell as anything in cells)
		if(cell?.pending_rebind && !cell.change_depth)
			cell.finish_rebind()
	for(var/datum/banda_atmos_cell/cell as anything in cells)
		if(cell)
			cell.refresh_context()
			cell.refresh_neighbors()
	running = TRUE
	var/success = synchronous ? native(16) : call_ext(library, "byond,await:banda_atmos_tick")(native_id)
	if(QDELETED(src))
		return
	running = FALSE
	if(!success)
		CRASH("Native atmosphere calculation failed")
	processing = TRUE
	try
		apply_frame()
	catch(var/exception/error)
		processing = FALSE
		throw error
	processing = FALSE

/datum/banda_atmos_phase/proc/apply_frame()
	SHOULD_NOT_SLEEP(TRUE)
	while(TRUE)
		while(length(waiting))
			var/datum/banda_atmos_safe/callback = waiting[1]
			waiting.Cut(1, 2)
			run_callback(callback)
		for(var/datum/banda_atmos_cell/cell as anything in cells)
			if(cell)
				cell.sync_air()
		var/list/commands = list()
		native(5, commands)
		if(!length(commands))
			break
		var/datum/banda_atmos_safe/world_commands/executor = new()
		executor.owner = src
		executor.run_args = list(commands)
		run_callback(executor)

/datum/banda_atmos_phase/proc/queue(datum/banda_atmos_safe/callback)
	if(closed)
		qdel(callback)
		return
	if(running)
		waiting += callback
	else
		run_callback(callback)

/datum/banda_atmos_phase/proc/run_callback(datum/banda_atmos_safe/callback)
	SHOULD_NOT_SLEEP(TRUE)
	if(QDELETED(callback))
		return
	var/previous_safe = in_safe_callback
	in_safe_callback = TRUE
	try
		callback.invoke()
	catch(var/exception/error)
		finish_callback(previous_safe, TRUE)
		qdel(callback)
		throw error
	finish_callback(previous_safe)
	qdel(callback)

/// Both queued effects and synchronous calls publish at the outermost boundary.
/datum/banda_atmos_phase/proc/finish_callback(previous_safe, failed = FALSE)
	SHOULD_NOT_SLEEP(TRUE)
	in_safe_callback = previous_safe
	if(previous_safe)
		return
	// Banda DM keeps completed writes after an exception; rolling back a debit can duplicate gas
	// that a receiver already consumed. The caller still stops the effect packet and rethrows.
	try
		for(var/datum/banda_atmos_cell/cell as anything in working_mixtures)
			if(!QDELETED(cell) && cell.target && cell.owner == src && !cell.pending_rebind)
				var/changed = cell.commit_air(working_mixtures[cell], synchronize = !world_owned, wake = !world_owned)
				if(world_owned && cell.readonly_air && (changed || cell.readonly_air.last_share != working_mixtures[cell].last_share))
					cell.copy_readonly(working_mixtures[cell])
		finish_gas_operations(FALSE)
	catch(var/exception/error)
		finish_gas_operations(TRUE)
		working_mixtures.Cut()
		throw error
	working_mixtures.Cut()

/// Nested callbacks share a single mutable mixture until the outer callback commits it.
/datum/banda_atmos_phase/proc/borrow_air(datum/banda_atmos_cell/cell)
	if(!in_safe_callback || running || !cell || cell.owner != src || cell.pending_rebind)
		CRASH("Mutable atmosphere access outside a safe phase")
	if(!working_mixtures[cell])
		working_mixtures[cell] = cell.read_air()
	return working_mixtures[cell]
