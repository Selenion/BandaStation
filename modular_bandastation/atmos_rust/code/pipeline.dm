/// Experimental one-calculation feedback delay, independently selectable for measurements.
/datum/config_entry/flag/rust_atmos_pipeline

/datum/banda_atmos_phase/world_adapter
	var/pipelined = FALSE
	/// Numeric commands only: neither queue retains a Rust frame or a game object.
	var/list/deferred_commands = list()
	var/list/active_commands = list()
	var/active_command_cursor = 1
	var/pipeline_calculations = 0
	var/pipeline_effect_packets = 0
	var/pipeline_empty_exposures = 0

/// Preparation runs in SSair; completion never resumes an MC-budgeted loop.
/datum/banda_atmos_phase/world_adapter/proc/start_pipeline_tick()
	SHOULD_NOT_SLEEP(TRUE)
	if(!pipelined || !threaded || initializing || closed || running || worker_cycle || processing || in_safe_callback || length(active_commands))
		CRASH("Pipeline atmosphere phase cannot overlap itself")
	if(length(uninitialized_cells))
		return FALSE
	collect_pipeline_commands()
	if(length(uninitialized_cells))
		return FALSE
	flush_topology()
	flush_inputs()
	active_commands = deferred_commands
	active_command_cursor = 1
	deferred_commands = list()
	tick_started = REALTIMEOFDAY
	running = TRUE
	worker_cycle = SSair.times_fired
	INVOKE_ASYNC(src, PROC_REF(finish_pipeline_calculation), worker_cycle)
	skip_pipeline_empty_exposures()
	return TRUE

/// This prefix has no callback, gas access or cache alias to flush; it can overlap numeric work.
/datum/banda_atmos_phase/world_adapter/proc/skip_pipeline_empty_exposures()
	SHOULD_NOT_SLEEP(TRUE)
	while(active_command_cursor <= length(active_commands))
		var/index = active_command_cursor
		if(active_commands[index + 2] != 13)
			break
		var/id = active_commands[index]
		var/turf/target = positions[id]
		var/datum/banda_atmos_cell/cell = length(cells) >= id ? cells[id] : null
		if(cell?.cached_air || cell?.readonly_air || cell?.pending_rebind || !isopenturf(target) || target._listen_lookup?[COMSIG_TURF_EXPOSE] || \
			(target.flags_1 & ATMOS_IS_PROCESSING_1) || target.should_atmos_process(null, active_commands[index + 3]))
			break
		active_command_cursor += 5
		pipeline_empty_exposures++
	if(active_command_cursor > length(active_commands))
		active_commands.Cut()

/datum/banda_atmos_phase/world_adapter/proc/finish_pipeline_calculation(cycle)
	var/success = call_ext(library, "byond,await:banda_atmos_world_tick")(native_id, cycle)
	// Keep even an immediately completed job outside the launching MC call's budget.
	sleep(0)
	if(QDELETED(src) || closed || worker_cycle != cycle)
		return
	try
		if(!success)
			running = FALSE
			native(10)
			CRASH("Native pipeline atmosphere calculation failed")
		synchronize_calculation()
		collect_pipeline_commands()
		publication_pending = FALSE
		completed_ticks++
		pipeline_calculations++
		apply_pipeline_effects()
	catch(var/exception/error)
		// A publication callback can fail before the packet is taken by apply_pipeline_effects.
		active_commands.Cut()
		worker_cycle = 0
		last_tick_ms = (REALTIMEOFDAY - tick_started) * 100
		throw error
	worker_cycle = 0
	last_tick_ms = (REALTIMEOFDAY - tick_started) * 100

/// Gas and issued aliases are published immediately; only physical feedback waits one calculation.
/datum/banda_atmos_phase/world_adapter/proc/collect_pipeline_commands()
	SHOULD_NOT_SLEEP(TRUE)
	var/previous_processing = processing
	processing = TRUE
	try
		project_changes()
		run_waiting_callbacks()
		project_changes()
		var/list/commands = list()
		native(5, commands)
		deferred_commands += commands
		refresh_debug_overlays()
	catch(var/exception/error)
		processing = previous_processing
		throw error
	processing = previous_processing

/// One callback keeps the completed prefix if a later effect throws, like ordinary Banda DM.
/datum/banda_atmos_phase/world_adapter/proc/apply_pipeline_effects()
	SHOULD_NOT_SLEEP(TRUE)
	var/list/commands = active_commands
	var/start = active_command_cursor
	active_commands = list()
	if(!length(commands) || closed)
		return
	// An opaque DM effect can request a live mixture: never overwrite fresh gas with an old sample.
	// Completion publishes fresh gas before invoking this atomic packet.
	synchronize_calculation()
	if(QDELETED(src) || closed)
		return
	var/datum/banda_atmos_safe/world_commands/world/executor = new()
	executor.owner = src
	executor.run_args = list(commands, start)
	var/previous_processing = processing
	processing = TRUE
	try
		run_callback(executor)
		pipeline_effect_packets++
	catch(var/exception/error)
		processing = previous_processing
		throw error
	processing = previous_processing
