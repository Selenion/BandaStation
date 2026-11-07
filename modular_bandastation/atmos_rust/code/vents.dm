/datum/banda_atmos_phase/world_adapter
	/// The DLL can access live port lists and return compact visuals for the receiving tile.
	var/vent_transfer_ready = FALSE
	/// Immediate transfers are allowed in the machinery callback that commits on error.
	var/vent_transfer_scope = FALSE
	/// Successful direct transfers, for local profiling.
	var/vent_transfers = 0
	/// Direct operations that found no transferable gas.
	var/vent_empty_transfers = 0
	/// Attempts that retained ordinary DM dispatch.
	var/vent_transfer_fallbacks = 0

/** Transfer against the live port. Null requests the original proc, zero means no gas moved. */
/datum/banda_atmos_phase/world_adapter/proc/try_vent_transfer(turf/open/target, datum/gas_mixture/port, offset, amount, releasing)
	SHOULD_NOT_SLEEP(TRUE)
	if(!vent_transfer_ready || !vent_transfer_scope)
		return null
	if(!offset || !SSair.initialized || running || closed || target.air || target.planetary_atmos || port?.type != /datum/gas_mixture || port.volume <= 0)
		vent_transfer_fallbacks++
		return null
	var/signal_type = releasing ? COMSIG_GASMIX_REMOVED : COMSIG_GASMIX_MERGED
	if(port._listen_lookup?[signal_type] || (releasing && port.moles == port.moles_archive))
		vent_transfer_fallbacks++
		return null
	var/datum/banda_atmos_pipe_binding/binding = owned_pipe_port(port)
	if(releasing && ((!binding && !length(port.moles)) || amount <= 0))
		vent_empty_transfers++
		return FALSE
	var/id = target.banda_atmos_id
	var/list/result = list()
	var/status
	var/reading_offset = 0
	if(binding)
		status = native(70, releasing ? id : -id, air_reading_packet[offset], binding.handle, amount, result)
		if(status != 1)
			port.temperature = result[1]
			binding.pressure = result[2]
			reading_offset = 2
			pipe_owned_transfers++
	else
		status = native(55, releasing ? id : -id, air_reading_packet[offset], port, banda_atmos_gases(), amount, result)
	// The native function returns before any mutation when it requests the DM path.
	if(status == 1)
		vent_transfer_fallbacks++
		return null
	if(length(result) > reading_offset)
		// Another device can use these values in the same callback, before the next publication.
		for(var/field in 1 to BANDA_ATMOS_HEAT_CAPACITY)
			air_reading_packet[offset + field] = result[reading_offset + field]
		banda_atmos_update_visual_states(target, result, reading_offset + 1)
		target.excited = TRUE
		target.significant_share_ticker = 0
	if(status == 3)
		vent_transfers++
		return TRUE
	vent_empty_transfers++
	return FALSE

/// Passive vents equalize two mixtures rather than applying a pump pressure bound.
/datum/banda_atmos_phase/world_adapter/proc/try_passive_vent(turf/open/target, datum/gas_mixture/port)
	SHOULD_NOT_SLEEP(TRUE)
	if(!vent_transfer_ready || !vent_transfer_scope || !SSair.initialized || target?.planetary_atmos || port?.type != /datum/gas_mixture || port.volume <= 0 || port.moles == port.moles_archive)
		return null
	var/version = cold_air_version(target)
	if(!version || target.banda_atmos_cell?.updating_hotspot)
		return null
	var/id = target.banda_atmos_id
	var/datum/banda_atmos_pipe_binding/binding = owned_pipe_port(port)
	var/status
	if(binding)
		var/list/readings = list()
		status = native(72, id, version, binding.handle, readings)
		if(status != 1)
			port.temperature = readings[1]
			binding.pressure = readings[2]
			pipe_owned_transfers++
	else
		status = native(67, id, version, port, banda_atmos_gases())
	if(status == 1)
		return null
	invalidate_air_reading(id)
	if(status == 3)
		wake_cold_tile(target)
	return status == 3

/datum/banda_atmos_phase/world_adapter
	/// The DLL can update both of a scrubber's already borrowed live mixtures.
	var/scrubber_transfer_ready = FALSE
	/// Direct operations that moved gas to the port.
	var/scrubber_transfers = 0
	/// Direct operations that cleaned zero entries or processed an empty purge.
	var/scrubber_empty_transfers = 0
	/// Operations that kept the original DM exchange.
	var/scrubber_transfer_fallbacks = 0
	/// Read key presence and operate on cold tile air without borrowing a DM mixture.
	var/scrubber_tile_ready = FALSE
	/// The outer safe callback still owns edits to one or more Rust tiles.
	var/scrubber_tile_pending = FALSE
	/// Operations completed without materializing the turf's DM mixture, for profiling.
	var/scrubber_tile_operations = 0
	var/scrubber_tile_moves = 0

/// Once a mixture is exposed to DM, callers can hold its lists and mutate them directly.
/datum/banda_atmos_phase/world_adapter/proc/scrubber_binding_version(turf/open/target)
	if(!scrubber_tile_ready || running || closed || initializing || !isopenturf(target) || !target.banda_air_present || target.air || target.planetary_atmos || !(target.flags_1 & INITIALIZED_1))
		return 0
	var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
	if(cell && (cell.pending_rebind || cell.cached_air || cell.readonly_air || cell.target != target || cell.owner != src))
		return 0
	return target.banda_atmos_id ? (cell?.binding_version || 1) : 0

/datum/banda_atmos_phase/world_adapter/proc/scrubber_air_keys(turf/open/target)
	var/version = scrubber_binding_version(target)
	if(!version)
		return null
	var/keys = native(59, target.banda_atmos_id, version)
	return keys > 0 ? keys - 1 : null

/// Null defers to the existing path. Zero means the port pressure prevented an exchange.
/datum/banda_atmos_phase/world_adapter/proc/try_scrubber_tile(turf/open/target, datum/gas_mixture/port, list/filters, purging, rate)
	SHOULD_NOT_SLEEP(TRUE)
	if(!in_safe_callback || !SSair.initialized || port?.type != /datum/gas_mixture || port._listen_lookup?[COMSIG_GASMIX_MERGED] || (!purging && isnull(filters)))
		return null
	var/version = scrubber_binding_version(target)
	if(!version || isspaceturf(target) || target.banda_atmos_cell?.updating_hotspot)
		return null
	var/datum/banda_atmos_pipe_binding/binding = owned_pipe_port(port)
	if(port.banda_pipe_pressure() >= 50 * ONE_ATMOSPHERE)
		return FALSE
	var/id = target.banda_atmos_id
	var/list/visuals = (purging || binding) ? list() : null
	var/status
	if(binding)
		status = native(71, list(id, version), binding.handle, banda_atmos_gases(), purging ? null : filters, rate, visuals)
		if(length(visuals) >= 2)
			port.temperature = visuals[1]
			binding.pressure = visuals[2]
			pipe_owned_transfers++
	else
		status = native(60, list(id, version), port, banda_atmos_gases(), purging ? null : filters, rate, visuals)
	if(status == 1)
		return null
	scrubber_tile_operations++
	if(status != 2)
		scrubber_tile_pending = TRUE
		invalidate_air_reading(id)
	if(status == 3)
		scrubber_tile_moves++
		scrubber_transfers++
	else if(status != 2)
		scrubber_empty_transfers++
	if(purging)
		// remove_air refreshes overlays and wakes the tile before the pipe notification.
		banda_atmos_update_visual_states(target, visuals, binding ? 2 : 0)
		wake_cold_tile(target)
	return status

/datum/banda_atmos_phase/world_adapter/proc/wake_cold_tile(turf/open/target)
	// The active queue already feeds terrain/context imports. mark_dirty would borrow air.
	target.excited = TRUE
	target.significant_share_ticker = 0
	native(19, target.banda_atmos_id, 0)

/// Ordinary access later in the callback must see and own the staged edit.
/datum/banda_atmos_phase/world_adapter/proc/materialize_scrubber_tile(datum/banda_atmos_cell/cell)
	if(cell.pending_rebind)
		return
	var/list/input = list()
	var/list/metadata = list()
	if(native(62, cell.native_id, cell.binding_version, input, metadata) != 2)
		return
	// Keep the published baseline so a later commit can detect changes to the working mixture.
	cell.refresh_cache()
	banda_atmos_decode_air(input, metadata, cell.native_air_kind, cell.cached_air, TRUE)
	working_mixtures[cell] = cell.cached_air
	dirty_gas[cell.target] = TRUE

/datum/banda_atmos_phase/world_adapter/proc/finish_scrubber_tiles(failed)
	if(failed)
		// The old callback leaves its edited DM cache visible until the next publication.
		// Materialize only on this exceptional path, without committing the gas to Rust.
		var/list/ids = list()
		native(61, FALSE, ids)
		for(var/id in ids)
			var/turf/target = positions[id]
			if(target?.banda_atmos_id == id)
				materialize_scrubber_tile(ensure_binding(target))
	native(61, !failed)
	scrubber_tile_pending = FALSE

/** Keep the tile in the caller's existing exposure callback and preserve its commit rules. */
/datum/banda_atmos_phase/world_adapter/proc/try_scrubber_transfer(turf/target, datum/gas_mixture/air, datum/gas_mixture/port, list/selected, rate)
	SHOULD_NOT_SLEEP(TRUE)
	var/turf/open/open = target
	var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
	if(!in_safe_callback || running || closed || !istype(open) || isspaceturf(open) || !cell || cell.owner != src || cell.target != target || cell.pending_rebind || working_mixtures[cell] != air)
		scrubber_transfer_fallbacks++
		return null
	if((air.type != /datum/gas_mixture/turf && air.type != /datum/gas_mixture) || port?.type != /datum/gas_mixture || air == port || air.moles == air.moles_archive || port._listen_lookup?[COMSIG_GASMIX_MERGED] || (isnull(selected) && air._listen_lookup?[COMSIG_GASMIX_REMOVED]))
		scrubber_transfer_fallbacks++
		return null
	var/status = native(57, air, port, banda_atmos_gases(), selected, rate, air.type == /datum/gas_mixture/turf)
	if(status == 1)
		scrubber_transfer_fallbacks++
		return null
	if(status == 3)
		scrubber_transfers++
	else
		scrubber_empty_transfers++
	return status
