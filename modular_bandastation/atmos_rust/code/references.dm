/// The caller owns this short-lived observation and deletes it on both success and error.
/datum/banda_atmos_air_reference
	var/datum/banda_atmos_phase/world_adapter/owner
	var/list/handle
	var/source_id
	var/source_version
	var/pending_version
	var/datum/gas_mixture/legacy_air

/datum/banda_atmos_air_reference/Destroy()
	release_native()
	legacy_air = null
	return ..()

/datum/banda_atmos_air_reference/proc/release_native()
	var/datum/banda_atmos_phase/world_adapter/phase = owner
	var/list/released = handle
	owner = null
	handle = null
	if(phase)
		phase.air_references -= src
		if(phase.native_id && released)
			phase.native(88, 1, released)

/// When an unconverted consumer borrows this mixture, retain its actual live object.
/datum/banda_atmos_air_reference/proc/use_legacy(datum/gas_mixture/air)
	legacy_air = air
	if(handle)
		var/list/released = handle
		handle = null
		owner.native(88, 1, released)

/datum/banda_atmos_air_reference/proc/native_values(list/gases, all_gases = FALSE)
	var/list/indices = list()
	for(var/gas in gases)
		var/index = banda_atmos_gases().Find(gas)
		if(!index)
			CRASH("Unknown gas in a native air reference")
		indices += index - 1
	var/list/output = list()
	owner.native(89, handle, indices, all_gases, output)
	var/gas_fields = all_gases ? output[BANDA_AIR_GAS_COUNT] * 2 : length(indices)
	if(length(output) != BANDA_AIR_GASES - 1 + gas_fields || output[1] != 1 || output[2] != 1 || (output[3] != 1 && output[3] != 2) || output[4] != handle[2])
		CRASH("Invalid native air reference")
	return output

/datum/banda_atmos_air_reference/proc/values(list/gases, all_gases = FALSE)
	SHOULD_NOT_SLEEP(TRUE)
	if(legacy_air)
		return banda_atmos_value_packet(legacy_air, gases, all_gases)
	var/list/output = native_values(gases, all_gases)
	if(output[3] == 1)
		var/datum/banda_atmos_cell/cell = length(owner.cells) >= source_id ? owner.cells[source_id] : null
		if(cell && cell.binding_version == source_version && !cell.pending_rebind && cell.cached_air)
			use_legacy(cell.cached_air)
			return banda_atmos_value_packet(legacy_air, gases, all_gases)
	owner.value_queries++
	return output

/datum/banda_atmos_air_reference/proc/total_moles()
	if(legacy_air)
		return legacy_air.total_moles()
	var/list/output = values()
	return output[BANDA_AIR_TOTAL_MOLES]

/// Key presence matters to consumers which distinguish a missing gas from a present zero.
/datum/banda_atmos_air_reference/proc/gas_moles(gas)
	if(legacy_air)
		return legacy_air.moles[gas]
	var/list/output = values(all_gases = TRUE)
	var/gas_id = banda_atmos_gases().Find(gas) - 1
	for(var/index in 1 to output[BANDA_AIR_GAS_COUNT])
		var/offset = BANDA_AIR_GASES + (index - 1) * 2
		if(output[offset] == gas_id)
			return output[offset + 1]
	return null

/atom/proc/return_air_reference()
	var/datum/gas_mixture/air = return_air()
	if(!air)
		return null
	var/datum/banda_atmos_air_reference/reference = new
	reference.legacy_air = air
	return reference

/turf/open/return_air_reference()
	var/datum/banda_atmos_phase/world_adapter/phase = SSair.native_atmos
	if(!phase?.air_references_ready || phase.initializing || phase.closed || phase.scrubber_tile_pending || !banda_air_present || air || !(flags_1 & INITIALIZED_1))
		return ..()
	var/datum/banda_atmos_cell/cell = banda_atmos_cell
	if(!banda_atmos_id || phase.positions[banda_atmos_id] != src || cell?.pending_rebind)
		return ..()
	if(cell && (cell.cached_air || cell.readonly_air))
		var/datum/banda_atmos_air_reference/legacy = ..()
		if(legacy)
			legacy.owner = phase
			legacy.source_id = banda_atmos_id
			legacy.source_version = cell.binding_version
			phase.air_references += legacy
		return legacy
	var/datum/banda_atmos_air_reference/reference = new
	reference.source_id = banda_atmos_id
	reference.source_version = cell?.binding_version || 1
	var/list/handle = list()
	phase.native(88, 0, list(0, reference.source_id, reference.source_version), handle)
	reference.owner = phase
	reference.handle = handle
	phase.air_references += reference
	return reference

/datum/banda_atmos_phase/world_adapter
	var/air_references_ready = FALSE
	/// Explicitly scoped references; close detaches their ownership before deleting native state.
	var/list/air_references = list()

/datum/banda_atmos_phase/world_adapter/proc/air_reference_count()
	var/list/output = list()
	native(88, 2, output)
	return output[1]

/datum/banda_atmos_phase/world_adapter/proc/prepare_air_references(datum/banda_atmos_cell/cell)
	if(!length(air_references))
		return
	for(var/datum/banda_atmos_air_reference/reference as anything in air_references.Copy())
		if(reference.source_id != cell.native_id || reference.source_version != cell.binding_version)
			continue
		if(reference.legacy_air)
			if(reference.legacy_air == cell.cached_air)
				cell.reference_old_air = cell.cached_air
			continue
		var/list/state = reference.native_values()
		if(state[3] != 1)
			continue
		if(cell.cached_air)
			cell.reference_old_air = cell.cached_air
			reference.use_legacy(cell.cached_air)
		else
			reference.pending_version = cell.binding_version + 1

/datum/banda_atmos_phase/world_adapter/proc/attach_pending_air_references(datum/banda_atmos_cell/cell, datum/gas_mixture/air)
	if(!length(air_references))
		return
	for(var/datum/banda_atmos_air_reference/reference as anything in air_references.Copy())
		if(reference.handle && reference.source_id == cell.native_id && reference.pending_version == cell.binding_version)
			reference.use_legacy(air)

/datum/banda_atmos_phase/world_adapter/proc/detach_air_references()
	for(var/datum/banda_atmos_air_reference/reference as anything in air_references.Copy())
		if(reference.legacy_air)
			reference.release_native()
			continue
		var/list/state = reference.native_values()
		var/turf/open/target = positions[reference.source_id]
		if(state[3] == 1 && istype(target) && target.air)
			reference.use_legacy(target.air)
		else
			var/list/air = list()
			var/list/archive = list()
			var/kind = native(90, reference.handle, air, archive)
			reference.use_legacy(banda_atmos_decode_air(air, archive, kind, compact = TRUE))
		reference.release_native()
