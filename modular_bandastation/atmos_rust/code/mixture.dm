/datum/gas_mixture/turf
	/// Only the current mixture of a world-owned turf has this back-reference.
	var/turf/open/banda_turf
	var/datum/banda_atmos_cell/banda_cell
	var/banda_version
	var/banda_readonly = FALSE

/datum/gas_mixture/turf/proc/banda_touch()
	if(banda_readonly)
		CRASH("Attempted to modify a read-only native atmosphere snapshot")
	if(banda_cell)
		if(QDELETED(banda_cell) || banda_cell.binding_version != banda_version || banda_cell.pending_rebind)
			CRASH("Attempted to modify an outdated native atmosphere binding")
		var/datum/banda_atmos_phase/phase = banda_cell.owner
		if(!phase?.in_safe_callback || phase.running)
			CRASH("Native atmosphere mutation outside a safe callback")
		phase.working_mixtures[banda_cell] = src
		return
	if(banda_turf?.air == src)
		SSair.native_atmos?.mark_dirty(banda_turf)

/datum/gas_mixture/turf/Destroy()
	banda_turf = null
	banda_cell = null
	return ..()

/datum/gas_mixture/turf/assert_gas(gas_id)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/assert_gases(...)
	banda_touch()
	return ..(arglist(args))

/datum/gas_mixture/turf/add_gas(gas_id)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/add_gases(...)
	banda_touch()
	return ..(arglist(args))

/datum/gas_mixture/turf/garbage_collect()
	banda_touch()
	return ..()

/datum/gas_mixture/turf/archive()
	banda_touch()
	return ..()

/datum/gas_mixture/turf/merge(datum/gas_mixture/giver)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/set_gas(gas_specie, amount)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/set_temperature(target_temp)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/adjust_gas(gas, amount)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/adjust_multiple_gases(list/gases_moles)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/convert_gas(datum/gas/reactant, datum/gas/product, conversion_amount)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/remove(amount)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/remove_ratio(ratio)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/remove_specific(gas_id, amount)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/remove_specific_ratio(gas_id, ratio)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/equalize(datum/gas_mixture/other)
	banda_touch()
	if(istype(other, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/turf_air = other
		turf_air.banda_touch()
	return ..()

/datum/gas_mixture/turf/copy_from(datum/gas_mixture/sample)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/copy_from_ratio(datum/gas_mixture/sample, partial = 1)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/share(datum/gas_mixture/sharer, our_coeff, sharer_coeff)
	banda_touch()
	if(istype(sharer, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/turf_air = sharer
		turf_air.banda_touch()
	return ..()

/datum/gas_mixture/turf/temperature_share(datum/gas_mixture/sharer, conduction_coefficient, sharer_temperature, sharer_heat_capacity)
	banda_touch()
	if(istype(sharer, /datum/gas_mixture/turf))
		var/datum/gas_mixture/turf/turf_air = sharer
		turf_air.banda_touch()
	return ..()

/datum/gas_mixture/turf/react(datum/holder)
	banda_touch()
	return ..()

/datum/gas_mixture/turf/electrolyze(working_power = 0, electrolyzer_args = list())
	banda_touch()
	return ..()
