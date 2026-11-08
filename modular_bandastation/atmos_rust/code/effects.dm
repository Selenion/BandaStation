/// Called only from SSair.fire: each invocation finishes its callback before MC can pause.
/datum/banda_atmos_phase/world_adapter/proc/process_effect_chunk()
	SHOULD_NOT_SLEEP(TRUE)
	if(!effects_ready || running || worker_cycle || processing || in_safe_callback)
		CRASH("Native effects require a completed frame and an ordinary MC phase")
	processing = TRUE
	try
		if(length(effect_commands))
			var/datum/banda_atmos_safe/world_commands/world/executor = new()
			executor.owner = src
			executor.cycle = air_cycle
			executor.run_args = list(effect_commands, effect_cursor, TRUE)
			effect_cursor = run_callback(executor)
			if(QDELETED(src) || closed)
				processing = FALSE
				return
			effect_chunks++
			if(effect_cursor <= length(effect_commands))
				refresh_effect_readonly()
				processing = FALSE
				return
			finish_effect_packet()
			// Feedback can queue more commands; those follow the original packet's entire tail.
			effect_commands = collect_effect_commands()
			effect_cursor = 1
			if(length(effect_commands))
				begin_effect_packet()
				processing = FALSE
				return
		else
			finish_effect_packet()
		effects_ready = FALSE
		completed_ticks++
		refresh_debug_overlays()
		last_tick_ms = (REALTIMEOFDAY - tick_started) * 100
	catch(var/exception/error)
		// The callback already kept its completed writes. Recheck earlier aliases, skip the tail.
		effects_ready = FALSE
		effect_commands.Cut()
		processing = FALSE
		finish_effect_packet()
		throw error
	processing = FALSE

/// Cross-turf list writes bypass gas setters; issued readers must be fresh before MC can yield.
/datum/banda_atmos_phase/world_adapter/proc/refresh_effect_readonly()
	SHOULD_NOT_SLEEP(TRUE)
	for(var/datum/banda_atmos_cell/cell as anything in effect_mixtures)
		var/datum/gas_mixture/air = effect_mixtures[cell]
		if(QDELETED(cell) || cell.owner != src || !cell.target || cell.pending_rebind || cell.cached_air != air || !cell.readonly_air)
			continue
		var/datum/gas_mixture/readonly = cell.readonly_air
		if(readonly.temperature != air.temperature || readonly.temperature_archived != air.temperature_archived || readonly.volume != air.volume || readonly.last_share != air.last_share || \
			!banda_atmos_same_gases(readonly.moles, air.moles) || !banda_atmos_same_gases(readonly.moles_archive, air.moles_archive) || !banda_atmos_same_gases(readonly.reaction_results, air.reaction_results))
			cell.copy_readonly(air)
