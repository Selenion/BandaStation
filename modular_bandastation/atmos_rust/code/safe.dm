/// One-shot callback for mixed read/write access between native calculations.
/datum/banda_atmos_safe
	var/datum/banda_atmos_phase/owner
	var/list/run_args = list()
	var/list/target_versions = list()

/datum/banda_atmos_safe/Destroy()
	owner = null
	run_args.Cut()
	target_versions.Cut()
	return ..()

/datum/banda_atmos_safe/proc/invoke_async(datum/banda_atmos_phase/phase, ...)
	if(owner)
		CRASH("Atmosphere callbacks are one-shot")
	owner = phase
	run_args = args.Copy(2)
	for(var/turf/target in run_args)
		var/datum/banda_atmos_cell/cell = target.banda_atmos_cell
		if(cell?.owner == phase)
			target_versions[target] = cell.binding_version
	phase.queue(src)

/datum/banda_atmos_safe/proc/on_run(...)
	CRASH("Override on_run for atmosphere callback")

/datum/banda_atmos_safe/proc/get_turf_air(turf/open/target)
	if(!owner.in_safe_callback || owner.running)
		CRASH("Mutable atmosphere access outside a safe phase")
	var/datum/banda_atmos_cell/cell = target?.banda_atmos_cell
	if(!cell || cell.owner != owner || cell.pending_rebind || (target_versions[target] && target_versions[target] != cell.binding_version))
		return null
	return owner.borrow_air(cell)

/datum/banda_atmos_safe/proc/invoke()
	SHOULD_NOT_SLEEP(TRUE)
	on_run(arglist(run_args))

/// Existing world code accesses turf.air directly as well as through return_air.
/// Expose the same safe mixture through both paths while applying a published frame.
/datum/banda_atmos_safe/world_commands/on_run(list/commands)
	for(var/datum/banda_atmos_cell/cell as anything in owner.cells)
		if(cell && !cell.pending_rebind && isopenturf(cell.target) && !cell.target.blocks_air)
			var/turf/open/open = cell.target
			owner.working_mixtures[cell] = open.air
	for(var/index = 1; index <= length(commands); index += 5)
		var/datum/banda_atmos_cell/cell = owner.cells[commands[index]]
		if(cell && cell.target && !QDELETED(cell) && !cell.pending_rebind && cell.binding_version == commands[index + 1])
			cell.execute(commands[index + 2], commands[index + 3], commands[index + 4])

/datum/banda_atmos_safe/expose/on_run(datum/banda_atmos_cell/cell, version, temperature, volume, strengthen)
	if(QDELETED(cell) || !cell.target || cell.owner != owner || cell.pending_rebind || cell.binding_version != version)
		return
	cell.refresh_context()
	owner.native(14, cell.native_id, temperature, volume, !!strengthen)
	cell.cache_current = FALSE
