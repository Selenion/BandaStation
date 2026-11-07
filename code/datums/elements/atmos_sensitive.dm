//This element facilitates reaction to atmos changes when a tile is inactive.
//It adds the object to a list on SSair to be processed for so long as the object wants to be processed
//And removes it as soon as the object is no longer interested
//Don't put it on things that tend to clump into one spot, you will cause lag spikes.
/datum/element/atmos_sensitive
	element_flags = ELEMENT_DETACH_ON_HOST_DESTROY

/datum/element/atmos_sensitive/Attach(datum/target, mapload)
	if(!isatom(target)) //How
		return ELEMENT_INCOMPATIBLE
	var/atom/to_track = target
	if(to_track.loc)
		to_track.RegisterSignal(to_track.loc, COMSIG_TURF_EXPOSE, TYPE_PROC_REF(/atom, check_atmos_process))
	RegisterSignal(to_track, COMSIG_MOVABLE_MOVED, PROC_REF(react_to_move))

	if(!mapload && isopenturf(to_track.loc))
		to_track.atmos_conditions_changed() //Make sure you're properly registered

	return ..()

/datum/element/atmos_sensitive/Detach(atom/source)
	if(source.loc)
		source.UnregisterSignal(source.loc, COMSIG_TURF_EXPOSE) // Attach registers this on the atom, not the element
	UnregisterSignal(source, COMSIG_MOVABLE_MOVED)
	if(source.flags_1 & ATMOS_IS_PROCESSING_1)
		source.atmos_end()
		SSair.atom_process -= source
		source.flags_1 &= ~ATMOS_IS_PROCESSING_1
	return ..()

/datum/element/atmos_sensitive/proc/react_to_move(atom/source, atom/movable/oldloc, direction, forced)
	SIGNAL_HANDLER

	if(oldloc)
		source.UnregisterSignal(oldloc, COMSIG_TURF_EXPOSE)
	if(source.loc)
		source.RegisterSignal(source.loc, COMSIG_TURF_EXPOSE, TYPE_PROC_REF(/atom, check_atmos_process))
	source.atmos_conditions_changed() //Make sure you're properly registered

/atom/proc/check_atmos_process(datum/source, datum/gas_mixture/air, exposed_temperature, native_keys) // BANDASTATION EDIT - Compact gas key presence for scrubbers.
	SIGNAL_HANDLER
	if(should_atmos_process(air, exposed_temperature, native_keys)) // BANDASTATION EDIT - Forward compact key presence.
		if(flags_1 & ATMOS_IS_PROCESSING_1)
			return
		SSair.atom_process += src
		flags_1 |= ATMOS_IS_PROCESSING_1
	else if(flags_1 & ATMOS_IS_PROCESSING_1)
		stop_atmos_exposure() // BANDASTATION EDIT - Share exposure cleanup with the native path.

// BANDASTATION ADDITION START - Share exposure cleanup with the native path.
/atom/proc/stop_atmos_exposure()
	atmos_end()
	SSair.atom_process -= src
	flags_1 &= ~ATMOS_IS_PROCESSING_1
// BANDASTATION ADDITION END

/atom/proc/process_exposure()
	var/turf/open/spot = loc
	if(!isopenturf(loc))
		//If you end up in a locker or a wall reconsider your life decisions
		stop_atmos_exposure() // BANDASTATION EDIT - Share exposure cleanup with the native path.
		return
	var/datum/gas_mixture/environment = spot.return_air() // BANDASTATION ADDITION - SSair executes exposure in a safe native phase.
	if(!should_atmos_process(environment, environment.temperature)) //Things can change without a tile becoming active // BANDASTATION EDIT
		stop_atmos_exposure() // BANDASTATION EDIT - Share exposure cleanup with the native path.
		return
	atmos_expose(environment, environment.temperature) // BANDASTATION EDIT - Exposure may modify native gas.

/turf/open/process_exposure()
	var/datum/gas_mixture/environment = return_air() // BANDASTATION ADDITION - Safe native gas access.
	if(!should_atmos_process(environment, environment.temperature)) // BANDASTATION EDIT
		atmos_end()
		SSair.atom_process -= src
		flags_1 &= ~ATMOS_IS_PROCESSING_1
		return
	atmos_expose(environment, environment.temperature) // BANDASTATION EDIT - Safe native gas access.

///We use this proc to check if we should start processing an item, or continue processing it. Returns true/false as expected
/atom/proc/should_atmos_process(datum/gas_mixture/air, exposed_temperature, native_keys) // BANDASTATION EDIT - Optional compact key presence.
	return FALSE

///This is your process() proc
/atom/proc/atmos_expose(datum/gas_mixture/air, exposed_temperature)
	return

///What to do when our requirements are no longer met
/atom/proc/atmos_end()
	return
