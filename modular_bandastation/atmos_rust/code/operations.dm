// Numeric values are shared with byond_operations.rs; operation_audit.py inventories the DM surface.
#define BANDA_GAS_MERGE 11
#define BANDA_GAS_TAKE 13
#define BANDA_GAS_ARCHIVE 9

/datum/banda_atmos_phase
	var/operations_ready = FALSE
	var/operation_transaction = FALSE
	var/list/operation_cells = list()

/datum/banda_atmos_phase/proc/finish_gas_operations(failed)
	if(!operation_transaction)
		return
	operation_transaction = FALSE
	if(failed)
		for(var/datum/banda_atmos_cell/cell as anything in operation_cells)
			if(!QDELETED(cell))
				cell.cache_current = FALSE
	try
		if(native_id && !closed)
			native(84, failed ? 2 : 1)
	catch(var/exception/error)
		for(var/datum/banda_atmos_cell/cell as anything in operation_cells)
			if(!QDELETED(cell))
				cell.cache_current = FALSE
		operation_cells.Cut()
		throw error
	operation_cells.Cut()

/// Only the operation input/new gas crosses into Rust. Its target remains a versioned native ID.
/datum/banda_atmos_phase/world_adapter/proc/gas_operations(datum/banda_atmos_cell/cell, list/commands)
	SHOULD_NOT_SLEEP(TRUE)
	if(!operations_ready || !in_safe_callback || running || closed || cell.owner != src || cell.pending_rebind)
		CRASH("Gas operation outside its current safe binding")
	if(!operation_transaction)
		native(84, 0)
		operation_transaction = TRUE
	var/list/output = list()
	native(83, list(0, cell.native_id, cell.binding_version), commands, FALSE, output)
	operation_cells[cell] = TRUE
	invalidate_air_reading(cell.native_id)
	return output

/// The compatible path retains all issued lists and gas-mixture signals.
/datum/banda_atmos_phase/world_adapter/proc/operation_cell(turf/open/target)
	if(!operations_ready || !in_safe_callback || running || closed || initializing || !target.banda_air_present || isspaceturf(target) || target.planetary_atmos)
		return null
	var/datum/banda_atmos_cell/cell = ensure_binding(target)
	if(!cell || cell.pending_rebind || cell.cached_air || cell.readonly_air || target.air)
		return null
	return cell

/// A callback requesting a live mixture takes over the pending state; it is committed by finish_callback.
/datum/banda_atmos_phase/world_adapter/proc/materialize_operation_cell(datum/banda_atmos_cell/cell)
	if(operation_cells[cell] != TRUE)
		return
	var/list/air = list()
	var/list/metadata = list()
	var/status = native(85, list(0, cell.native_id, cell.binding_version), air, metadata)
	if(status == 2)
		cell.refresh_cache(air, metadata, TRUE)
		// The returned state is still tentative; the ordinary outer commit must write it once.
		cell.forget_committed_air()
		operation_cells[cell] = 2
		working_mixtures[cell] = cell.cached_air
	else
		operation_cells -= cell

/datum/banda_atmos_phase/world_adapter/proc/try_spawn_air_operation(turf/open/target, text)
	var/datum/banda_atmos_cell/cell = operation_cell(target)
	if(!cell)
		return FALSE
	var/datum/gas_mixture/addition = SSair.parse_gas_string(text, /datum/gas_mixture/turf)
	var/list/packet = banda_atmos_air_packet(addition)
	var/list/commands = list(1, 2, BANDA_GAS_MERGE, length(packet))
	commands += packet
	commands += list(BANDA_GAS_ARCHIVE, 0)
	gas_operations(cell, commands)
	target.archived_cycle = SSair.times_fired
	target.temperature_archived = target.temperature
	native(37, cell.native_id, target.archived_cycle, target.temperature_archived)
	SSair.add_to_active(target)
	return TRUE

/datum/banda_atmos_phase/world_adapter/proc/try_assume_air_operation(turf/open/target, datum/gas_mixture/addition)
	// A source override can change heat capacity or attach gameplay to its reads.
	if(addition.type != /datum/gas_mixture && addition.type != /datum/gas_mixture/turf && addition.type != /datum/gas_mixture/turf/banda_plain_snapshot)
		return FALSE
	var/datum/banda_atmos_cell/cell = operation_cell(target)
	if(!cell)
		return FALSE
	var/list/packet = banda_atmos_air_packet(addition)
	var/list/commands = list(1, 1, BANDA_GAS_MERGE, length(packet))
	commands += packet
	gas_operations(cell, commands)
	target.update_visuals()
	target.air_update_turf(FALSE, FALSE)
	return TRUE

/// A non-null list means handled; its element may be null when no gas was available.
/datum/banda_atmos_phase/world_adapter/proc/try_remove_air_operation(turf/open/target, amount)
	if(!isnull(amount) && !isnum(amount))
		return null
	var/datum/banda_atmos_cell/cell = operation_cell(target)
	if(!cell)
		return null
	var/list/output = gas_operations(cell, list(1, 1, BANDA_GAS_TAKE, 1, amount || 0))
	var/datum/gas_mixture/removed
	if(output[4])
		switch(output[7])
			if(0)
				removed = new /datum/gas_mixture(output[6])
			if(1)
				removed = new /datum/gas_mixture/turf(output[6])
			if(2)
				removed = new /datum/gas_mixture/immutable/space(output[6])
			if(3)
				removed = new /datum/gas_mixture/immutable/planetary(output[6])
		removed.temperature = output[5]
		var/list/gases = banda_atmos_gases()
		var/index = 9
		for(var/entry in 1 to output[8])
			removed.moles[gases[output[index] + 1]] = output[index + 1]
			index += 2
	target.update_visuals()
	target.air_update_turf(FALSE, FALSE)
	return list(removed)
