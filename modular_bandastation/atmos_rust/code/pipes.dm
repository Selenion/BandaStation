// Pipe records currently synchronize with DM devices at each ordered reconcile boundary.
/datum/banda_atmos_phase
	var/pipes_ready = FALSE
	var/pipe_direct_ready = FALSE
	var/pipe_owned_ready = FALSE
	/// Weak references allow ordinary gas datums to be collected without holding the map alive.
	var/list/pipe_bindings = list()
	var/pipe_reap_index = 1
	var/pipe_groups = 0
	var/pipe_fallbacks = 0
	var/pipe_imports = 0
	var/pipe_direct_groups = 0
	var/pipe_packet_imports = 0
	var/pipe_owned_groups = 0
	var/pipe_owned_ports = 0
	var/pipe_owned_exposures = 0
	var/pipe_owned_transfers = 0

/datum/gas_mixture
	var/datum/banda_atmos_pipe_binding/banda_pipe_binding
	/// Cleared before this port's live gas can escape to a legacy caller.
	var/banda_native_port = FALSE

/datum/banda_atmos_pipe_binding
	var/datum/banda_atmos_phase/owner
	var/datum/weakref/mixture_ref
	var/list/handle
	var/native_owned = FALSE
	var/pressure = 0

/datum/banda_atmos_pipe_binding/New(datum/banda_atmos_phase/phase, datum/gas_mixture/mixture)
	..()
	owner = phase
	mixture_ref = WEAKREF(mixture)
	handle = list()
	owner.native(42, list(mixture.last_share) + banda_atmos_full_packet(mixture), handle)
	owner.pipe_packet_imports++
	mixture.banda_pipe_binding = src
	owner.pipe_bindings += src
	if(owner.pipe_owned_ready && owner.pipe_direct_ready && mixture.banda_native_port && !length(mixture._listen_lookup) && mixture.moles != mixture.moles_archive)
		pressure = mixture.return_pressure()
		native_owned = TRUE
		owner.pipe_owned_ports++
		mixture.moles.Cut()
		mixture.moles_archive.Cut()
		mixture.reaction_results.Cut()

/datum/banda_atmos_pipe_binding/proc/restore(datum/gas_mixture/mixture)
	if(!native_owned)
		return
	var/list/air = list()
	var/list/archive = list()
	owner.native(69, handle, air, archive)
	banda_atmos_decode_air(air, archive, 1, mixture, TRUE)
	native_owned = FALSE
	owner.pipe_owned_exposures++

/datum/banda_atmos_pipe_binding/Destroy()
	var/datum/gas_mixture/mixture = mixture_ref?.resolve()
	if(mixture?.banda_pipe_binding == src)
		if(!QDELETED(mixture))
			restore(mixture)
		mixture.banda_pipe_binding = null
	if(owner?.native_id && length(handle))
		owner.native(46, handle)
	owner?.pipe_bindings.Remove(src)
	owner = null
	mixture_ref = null
	handle = null
	return ..()

/datum/gas_mixture/Destroy()
	QDEL_NULL(banda_pipe_binding)
	return ..()

/datum/gas_mixture/proc/banda_expose_pipe_port()
	banda_native_port = FALSE
	banda_pipe_binding?.restore(src)

/datum/gas_mixture/proc/banda_pipe_pressure()
	return banda_pipe_binding?.native_owned ? banda_pipe_binding.pressure : return_pressure()

/datum/banda_atmos_phase/proc/bind_pipe_mixture(datum/gas_mixture/mixture)
	var/datum/banda_atmos_pipe_binding/binding = mixture.banda_pipe_binding
	if(binding && binding.owner != src)
		qdel(binding)
		binding = null
	if(!binding)
		binding = new(src, mixture)
	return binding

/datum/banda_atmos_phase/proc/owned_pipe_port(datum/gas_mixture/mixture)
	if(!pipes_ready || !pipe_owned_ready || !pipe_direct_ready || mixture?.type != /datum/gas_mixture || !mixture.banda_native_port)
		return null
	if(length(mixture._listen_lookup) || mixture.moles == mixture.moles_archive)
		mixture.banda_expose_pipe_port()
		return null
	var/datum/banda_atmos_pipe_binding/binding = bind_pipe_mixture(mixture)
	return binding.native_owned ? binding : null

/// GC does not call Destroy(); periodically reap weak bindings as well as explicit qdel paths.
/datum/banda_atmos_phase/proc/reap_pipe_bindings()
	for(var/checked in 1 to min(256, length(pipe_bindings)))
		if(pipe_reap_index > length(pipe_bindings))
			pipe_reap_index = 1
		var/datum/banda_atmos_pipe_binding/binding = pipe_bindings[pipe_reap_index]
		var/datum/gas_mixture/mixture = binding.mixture_ref?.resolve()
		if(!mixture || QDELETED(mixture))
			qdel(binding)
		else
			pipe_reap_index++

/datum/banda_atmos_phase/proc/reconcile_pipes(list/mixtures)
	SHOULD_NOT_SLEEP(TRUE)
	if(length(mixtures) > 65536)
		pipe_fallbacks++
		return FALSE
	// Overrides may attach effects to copy_from_ratio; retain their ordinary dispatch.
	for(var/datum/gas_mixture/mixture as anything in mixtures)
		if(!mixture || mixture.type != /datum/gas_mixture)
			pipe_fallbacks++
			return FALSE
	var/list/unique = list()
	var/list/handles = list()
	var/list/owned_rows
	for(var/datum/gas_mixture/mixture as anything in mixtures)
		if(unique[mixture])
			continue
		unique[mixture] = TRUE
		var/already_bound = mixture.banda_pipe_binding?.owner == src
		if(mixture.banda_native_port && length(mixture._listen_lookup))
			mixture.banda_expose_pipe_port()
		var/datum/banda_atmos_pipe_binding/binding = bind_pipe_mixture(mixture)
		if(binding.native_owned && pipe_direct_ready)
			LAZYADD(owned_rows, length(unique))
		else if(already_bound && !pipe_direct_ready)
			mixture.banda_expose_pipe_port()
			native(43, binding.handle, list(mixture.last_share) + banda_atmos_full_packet(mixture))
			pipe_packet_imports++
		if(!binding.native_owned)
			pipe_imports++
		handles += binding.handle
	if(pipe_direct_ready)
		if(length(owned_rows))
			var/list/objects = unique.Copy()
			var/list/readings = list()
			for(var/row in owned_rows)
				objects[row] = null
			native(68, handles, objects, banda_atmos_gases(), readings)
			var/index = 1
			for(var/row in owned_rows)
				var/datum/gas_mixture/port = unique[row]
				port.temperature = readings[index++]
				port.banda_pipe_binding.pressure = readings[index++]
			pipe_owned_groups++
		else
			native(48, handles, unique, banda_atmos_gases())
		pipe_direct_groups++
		pipe_groups++
		return TRUE
	var/list/output = list()
	native(45, handles, output)
	var/index = 1
	var/list/gases = banda_atmos_gases()
	for(var/datum/gas_mixture/mixture as anything in unique)
		var/temperature = output[index++]
		var/count = output[index++]
		// Preserve the live moles list and every field copy_from_ratio leaves untouched.
		mixture.moles.Cut()
		mixture.temperature = temperature
		for(var/entry in 1 to count)
			// DM evaluates the assignment's right side before its indexed left side.
			var/gas = gases[output[index++] + 1]
			var/amount = output[index++]
			mixture.moles[gas] = amount
	pipe_groups++
	return TRUE
