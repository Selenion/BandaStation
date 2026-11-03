/// Presentation only: never joins the legacy DM processing queue or owns turf physics.
/datum/excited_group/banda_native_view
	var/native_key
	var/tracked_max = 0

/datum/excited_group/banda_native_view/New(key)
	// The parent registers a simulation group; Rust already owns that group.
	native_key = key

/datum/excited_group/banda_native_view/Destroy()
	if(display_id)
		hide_turfs()
	turf_list.Cut()
	return ..()

/datum/banda_atmos_phase/world_adapter
	var/list/debug_groups = list()

/datum/banda_atmos_phase/world_adapter/proc/debug_state(detail = FALSE)
	var/list/output = list()
	native(31, !!detail, output)
	return output

/// Resolve coordinates to turfs only when an administrator requests group details/overlays.
/datum/banda_atmos_phase/world_adapter/proc/refresh_debug_groups()
	var/list/state = debug_state(TRUE)
	var/list/previous = list()
	for(var/datum/excited_group/banda_native_view/group as anything in debug_groups)
		previous[group.native_key] = group
	var/list/current = list()
	var/index = 6
	for(var/entry in 1 to state[3])
		var/key = "[state[index++]],[state[index++]],[state[index++]],[state[index++]]"
		var/datum/excited_group/banda_native_view/group = previous[key]
		if(!group)
			group = new(key)
		previous -= key
		var/color_id = group.display_id
		if(color_id)
			group.hide_turfs()
		group.breakdown_cooldown = state[index++]
		group.dismantle_cooldown = state[index++]
		var/count = state[index++]
		group.tracked_max = state[index++]
		group.turf_list.Cut()
		for(var/member in 1 to count)
			var/turf/target = positions[state[index++]]
			if(target && !target.banda_atmos_cell?.pending_rebind)
				group.turf_list += target
		if(group.should_display || SSair.display_all_groups)
			group.display_id = color_id
			group.display_turfs()
		current += group
	for(var/key in previous)
		qdel(previous[key])
	debug_groups = current
	return state

/datum/banda_atmos_phase/world_adapter/proc/debug_ui_data(list/data)
	var/list/state = refresh_debug_groups()
	data["active_size"] = state[1]
	data["hotspots_size"] = state[2]
	data["excited_size"] = state[3]
	data["conducting_size"] = state[4]
	var/list/rows = list()
	for(var/datum/excited_group/banda_native_view/group as anything in debug_groups)
		if(!length(group.turf_list))
			continue
		var/turf/target = group.turf_list[1]
		var/area/location = get_area(target)
		rows += list(list(
			"jump_to" = REF(target), "group" = REF(group), "area" = location.name,
			"breakdown" = group.breakdown_cooldown, "dismantle" = group.dismantle_cooldown,
			"size" = length(group.turf_list), "should_show" = group.should_display,
			"max_share" = group.tracked_max,
		))
	data["excited_groups"] = rows

/// The source UI checks R_DEBUG before forwarding an action. Revalidate ownership after refresh.
/datum/banda_atmos_phase/world_adapter/proc/debug_toggle_group(reference)
	refresh_debug_groups()
	var/datum/excited_group/banda_native_view/group = locate(reference) in debug_groups
	if(!group)
		return FALSE
	group.should_display = !group.should_display
	if(SSair.display_all_groups || group.should_display)
		group.display_turfs()
	else if(group.display_id)
		group.hide_turfs()
	return TRUE

/datum/banda_atmos_phase/world_adapter/proc/debug_toggle_all()
	SSair.display_all_groups = !SSair.display_all_groups
	refresh_debug_groups()
	return TRUE

/datum/banda_atmos_phase/world_adapter/proc/refresh_debug_overlays()
	if(SSair.display_all_groups)
		refresh_debug_groups()
		return
	for(var/datum/excited_group/banda_native_view/group as anything in debug_groups)
		if(group.should_display)
			refresh_debug_groups()
			return
