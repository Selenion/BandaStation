/datum/banda_atmos_cell/proc/environment(active)
	return banda_atmos_environment(target, active)

/// Destruction callbacks still see the old turf's gas, without writing into the replacement binding.
/datum/banda_atmos_cell/proc/read_pending_air()
	if(!isopenturf(target))
		return null
	var/turf/open/open = target
	// A new or uninitialized replacement must never borrow its predecessor's mixture.
	if(!open.air && pending_rebind && change_depth && open.banda_air_present && (open.flags_1 & INITIALIZED_1))
		open.air = read_air()
	return open.air

/datum/banda_atmos_cell/proc/turf_changing(turf/source, path, list/new_baseturfs, flags, list/post_change_callbacks)
	SIGNAL_HANDLER
	if(!owner || QDELETED(owner))
		return
	pending_archive_cycle = null
	pending_archive_temperature = null
	var/datum/gas_mixture/working = owner.working_mixtures[src]
	if(working)
		// Accepted feedback precedes the replacement; only its old world effects are invalidated.
		commit_air(working, FALSE, !owner.world_owned)
		owner.working_mixtures -= src
	var/inheriting = isopenturf(source) && ispath(path, /turf/open) && (flags & CHANGETURF_INHERIT_AIR)
	if(!pending_rebind)
		inherit_group = TRUE
		inherit_gas = TRUE
		pending_conduction = FALSE
		pending_control = null
	inherit_group = inherit_group && inheriting
	inherit_gas = inherit_gas && inheriting && !ispath(path, /turf/open/space)
	if(!inheriting)
		pending_control = null
	pending_rebind = TRUE
	change_depth++
	binding_version = owner.native(20, native_id)
	post_change_callbacks += CALLBACK(src, PROC_REF(turf_constructed), binding_version)
	cache_current = FALSE
	committed_air = null
	cached_air = null
	readonly_air = null

/datum/banda_atmos_cell/proc/turf_constructed(version, turf/replacement)
	if(QDELETED(src) || !owner || QDELETED(owner) || version != binding_version)
		return
	target = replacement
	replacement.banda_atmos_cell = src
	if(owner.world_owned)
		replacement.banda_atmos_id = native_id

/// Called after the original ChangeTurf path has restored its inherited atmosphere.
/datum/banda_atmos_cell/proc/turf_change_finished()
	if(!pending_rebind || !change_depth)
		return
	change_depth--
	if(change_depth)
		return
	request_rebind()

/// Map templates construct turfs before SSatoms initializes them and creates their air.
/datum/banda_atmos_cell/proc/request_rebind()
	if(!(target.flags_1 & INITIALIZED_1))
		owner.uninitialized_cells[src] = TRUE
		return
	if(owner.running || owner.processing || owner.in_safe_callback)
		var/datum/banda_atmos_safe/rebind/callback = new()
		callback.owner = owner
		callback.run_args = list(src, binding_version)
		owner.waiting += callback
	else
		finish_rebind()

/datum/banda_atmos_cell/proc/turf_initialized(turf/source)
	SIGNAL_HANDLER
	if(!owner || QDELETED(owner) || source != target || !pending_rebind || change_depth)
		return
	request_rebind()

/datum/banda_atmos_cell/proc/finish_rebind(force = FALSE)
	SHOULD_NOT_SLEEP(TRUE)
	if(!pending_rebind || change_depth || !target || !owner || (QDELETED(owner) && !force))
		return
	if(!(target.flags_1 & INITIALIZED_1))
		owner.uninitialized_cells[src] = TRUE
		return
	var/turf/open/open = isopenturf(target) ? target : null
	if(!open?.air)
		open = null
	var/datum/gas_mixture/air = open ? open.air : new /datum/gas_mixture(CELL_VOLUME)
	if(!open)
		air.temperature = TCMB
	if(!air || (istype(air, /datum/gas_mixture/immutable) && !istype(air, /datum/gas_mixture/immutable/space)))
		CRASH("Unsupported atmosphere after a native turf replacement")
	// A synchronous world has no unfinished gas frame; DM already restored inherited air
	// and may have modified it again inside the current effect callback.
	var/list/metadata = list(!!inherit_group, !!inherit_gas && !owner.world_owned)
	metadata += environment(open?.excited)
	owner.native(21, native_id, binding_version, commit_packet(air), context_bits(), freeze_permafrost(), metadata)
	if(!isnull(pending_archive_cycle))
		// An explicit archive on this replacement must survive its deferred native reattachment.
		owner.native(37, native_id, pending_archive_cycle, pending_archive_temperature)
		pending_archive_cycle = null
		pending_archive_temperature = null
	if(owner.world_owned)
		native_air_kind = owner.native(32, native_id)
	pending_rebind = FALSE
	owner.uninitialized_cells -= src
	if(pending_conduction)
		owner.native(19, native_id, 4)
	if(!isnull(pending_control))
		// Reattachment restores the old group first; later explicit activation must win.
		owner.native(19, native_id, pending_control)
		pending_control = null
	if(owner.world_owned)
		var/datum/banda_atmos_phase/world_adapter/world_owner = owner
		world_owner.handoff_air(target)
		world_owner.mark_topology(target)
		if(open?.active_hotspot)
			world_owner.adopt_hotspot(open.active_hotspot)
		if(hotspots_pending)
			adopt_pending_hotspots()
	sync_air()

/datum/banda_atmos_safe/rebind/on_run(datum/banda_atmos_cell/cell, version)
	if(QDELETED(cell) || cell.owner != owner || cell.binding_version != version)
		return
	cell.finish_rebind()
