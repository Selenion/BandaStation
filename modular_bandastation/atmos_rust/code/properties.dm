/datum/banda_atmos_phase/world_adapter
	/// Capability of the optional scalar-reading operation in the installed DLL.
	var/properties_ready = FALSE
	/// Turf to row in the scalar response, scoped to one non-sleeping machinery chunk.
	var/list/air_readings = list()
	/// Requested IDs in the same row order, used to detect changes to a turf's registration.
	var/list/air_reading_ids
	/// Retain the flat response rather than allocating separate DM lists for each row.
	var/list/air_reading_packet
	/// Completed bounded reading batches for local profiling.
	var/property_batches = 0
	/// Scalar getters served without a full mixture.
	var/property_hits = 0
	/// Scalar getters that retained the existing mixture path.
	var/property_fallbacks = 0
	/// Capability of the optional overlay-descriptor publication operation.
	var/visual_publication_ready = FALSE
	/// Ordinary icon refreshes can read compact descriptors independently of publication.
	var/visual_refresh_enabled = TRUE
	/// Compact icon refreshes, for local profiling.
	var/visual_refreshes = 0
	/// Numbers returned by either publication format for local profiling.
	var/publication_values = 0
	/// Published cells that did not need full gas contents in DM.
	var/visual_publications = 0
	/// Repeated cold service rows whose fields and presentation already match DM.
	var/service_publications_skipped = 0

/** Send Banda's actual overlay thresholds once; image references remain in DM. */
/datum/banda_atmos_phase/world_adapter/proc/configure_visual_publication()
	var/list/rules = list()
	var/list/minimums = GAS_META[META_GAS_MOLES_VISIBLE]
	for(var/gas in banda_atmos_gases())
		rules += GLOB.nonoverlaying_gases[gas] ? -1 : (minimums[gas] || 0)
	rules += list(MOLES_GAS_VISIBLE_STEP, TOTAL_VISIBLE_STATES)
	native(52, rules)
	visual_publication_ready = TRUE

/** Refresh a Rust-owned turf's overlays without creating a persistent DM mixture. */
/datum/banda_atmos_phase/world_adapter/proc/try_refresh_visuals(turf/open/target)
	if(!visual_refresh_enabled || !visual_publication_ready || initializing || closed || running || scrubber_tile_pending || target.air || !target.banda_air_present || !(target.flags_1 & INITIALIZED_1))
		return FALSE
	var/id = target.banda_atmos_id
	if(!id || id > length(positions) || positions[id] != target)
		return FALSE
	var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
	if(cell && (cell.cached_air || cell.readonly_air || cell.pending_rebind || cell.owner != src || cell.target != target))
		return FALSE
	var/list/output = list()
	native(53, list(id, FALSE), output)
	// The single publication record starts with ID, binding, coordinates, kind, visual flag and payload length.
	if(output[1] != id || output[2] != (cell?.binding_version || 1) || output[6] == 4 || output[7] != 1)
		return FALSE
	banda_atmos_update_visual_states(target, output, 8)
	visual_refreshes++
	return TRUE

/** Release the bounded readings at the end of their non-sleeping scope. */
/datum/banda_atmos_phase/world_adapter/proc/clear_air_readings()
	air_readings.Cut()
	air_reading_ids = null
	air_reading_packet = null

/** A native write makes earlier scalar readings of this coordinate stale. */
/datum/banda_atmos_phase/world_adapter/proc/invalidate_air_reading(id)
	if(length(air_readings) && isnum(id) && id == round(id) && id >= 1 && id <= length(positions))
		air_readings.Remove(positions[id])

/// The full access path stays responsible for live DM aliases and pending writes.
/datum/banda_atmos_phase/world_adapter/proc/cold_air_version(turf/open/target)
	if(!compact_exchange || initializing || running || closed || scrubber_tile_pending || !istype(target) || !target.banda_air_present || target.air || !(target.flags_1 & INITIALIZED_1) || isspaceturf(target))
		return 0
	var/id = target.banda_atmos_id
	if(!id || id > length(positions) || positions[id] != target)
		return 0
	var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
	if(cell && (cell.pending_rebind || cell.cached_air || cell.readonly_air || cell.owner != src || cell.target != target))
		return 0
	return cell?.binding_version || 1

/datum/banda_atmos_phase/world_adapter/proc/read_air_temperature_and_gas(turf/open/target, gas)
	var/version = cold_air_version(target)
	if(!version)
		return null
	var/gas_index = banda_atmos_gases().Find(gas)
	if(!gas_index)
		return null
	var/list/output = list()
	native(66, target.banda_atmos_id, version, gas_index - 1, output)
	return length(output) == 2 ? output : null

/** Read published numbers for the next machinery chunk without creating full mixtures. */
/datum/banda_atmos_phase/world_adapter/proc/prefetch_air_readings(list/machines)
	clear_air_readings()
	if(!properties_ready || !in_safe_callback || running || closed)
		return
	var/list/ids = list()
	var/last = length(machines)
	var/first = max(1, last - BANDA_ATMOS_READING_BATCH_SIZE + 1)
	for(var/index = last; index >= first; index--)
		var/obj/machinery/machine = machines[index]
		if(!istype(machine, /obj/machinery/atmospherics/components/unary/vent_pump) && !istype(machine, /obj/machinery/atmospherics/pipe/heat_exchanging))
			continue
		var/turf/open/target = machine.loc
		if(!istype(target) || !target.banda_atmos_id || !target.banda_air_present || target.blocks_air || !(target.flags_1 & INITIALIZED_1))
			continue
		var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
		if(cell && (cell.pending_rebind || cell.cached_air || cell.readonly_air))
			continue
		var/id = target.banda_atmos_id
		if(id > length(positions) || positions[id] != target)
			continue
		if(air_readings[target])
			continue
		ids += id
		air_readings[target] = length(ids)
	if(!length(ids))
		return
	var/list/output = list()
	native(51, ids, output)
	if(length(output) != 3 + length(ids) * 6 || output[1] != 1 || output[3] != length(ids))
		CRASH("Invalid native atmosphere property packet")
	air_reading_ids = ids
	air_reading_packet = output
	property_batches++

/** Return the offset for BANDA_ATMOS_* fields in air_reading_packet, or zero for a full read. */
/datum/banda_atmos_phase/world_adapter/proc/air_reading_offset(turf/open/target)
	var/row = air_readings[target]
	if(!row || !air_reading_packet || air_reading_ids[row] != target.banda_atmos_id || !in_safe_callback || running || closed || !target.banda_air_present || !(target.flags_1 & INITIALIZED_1))
		property_fallbacks++
		return 0
	// Three header values, six per row; status and binding version precede the readings.
	var/offset = 3 + (row - 1) * 6 + 2
	var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
	if(air_reading_packet[offset - 1] != 1 || (cell && (cell.pending_rebind || cell.binding_version != air_reading_packet[offset] || cell.cached_air || cell.readonly_air)))
		property_fallbacks++
		return 0
	property_hits++
	return offset

/datum/banda_atmos_phase/world_adapter/finish_callback(previous_safe, failed = FALSE)
	if(!previous_safe)
		if(scrubber_tile_pending)
			finish_scrubber_tiles(failed)
		clear_air_readings()
	return ..()
