/proc/banda_atmos_air_packet(datum/gas_mixture/air, archived = FALSE)
	var/list/gases = banda_atmos_gases()
	var/list/moles = archived ? air.moles_archive : air.moles
	var/list/output = list(archived ? air.temperature_archived : air.temperature, air.volume, istype(air, /datum/gas_mixture/turf) && !istype(air, /datum/gas_mixture/turf/banda_plain_snapshot), length(moles))
	for(var/gas in moles)
		var/index = gases.Find(gas)
		if(!index)
			CRASH("Gas missing from native atmosphere catalog")
		output += list(index - 1, moles[gas] || 0)
	return output

/proc/banda_atmos_full_packet(datum/gas_mixture/air)
	var/static/list/result_offsets
	var/static/list/empty_results
	if(!result_offsets)
		result_offsets = list()
		empty_results = list()
		var/offset = 1
		for(var/reaction in banda_atmos_reactions())
			result_offsets[reaction] = offset
			empty_results += list(0, 0)
			offset += 2
	var/list/output = banda_atmos_air_packet(air)
	// Ordinary equipment usually has no results: copy zeros instead of allocating 26 pairs.
	var/list/results = empty_results.Copy()
	for(var/reaction in air.reaction_results)
		var/offset = result_offsets[reaction]
		var/value = air.reaction_results[reaction]
		if(!offset || isnull(value))
			continue
		results[offset] = 1
		results[offset + 1] = value
	output += results
	output += banda_atmos_air_packet(air, TRUE)
	return output

/// Initial map mixtures normally have no reaction results; avoid expanding absent results.
/proc/banda_atmos_compact_packet(datum/gas_mixture/air, filter_results = FALSE)
	var/list/mix = banda_atmos_air_packet(air)
	var/list/output = list(length(mix)) + mix
	var/list/results = list()
	var/list/reactions = banda_atmos_reactions()
	for(var/reaction in air.reaction_results)
		var/index = reactions.Find(reaction)
		// World commits retain full_packet's handling of foreign and explicitly null keys.
		if(filter_results && (!index || isnull(air.reaction_results[reaction])))
			continue
		if(!index)
			CRASH("Reaction missing from native atmosphere catalog")
		results += list(index - 1, filter_results ? air.reaction_results[reaction] : (air.reaction_results[reaction] || 0))
	output += length(results) / 2
	output += results
	var/list/archive = banda_atmos_air_packet(air, TRUE)
	output += length(archive)
	output += archive
	return output

/proc/banda_atmos_context_bits(turf/target, rng_bits)
	return (isnull(rng_bits) ? 512 : rng_bits) | (isgroundlessturf(target) ? 4 : 0) | (isnoslipturf(target) ? 8 : 0) | \
		((locate(/obj/effect/particle_effect/fluid/foam) in target) ? 16 : 0) | \
		((locate(/obj/structure/foamedmetal/resin) in target) ? 32 : 0) | (target.thermal_conductivity ? 256 : 0)

/proc/banda_atmos_freeze_permafrost(turf/target)
	var/datum/component/wet_floor/floor = target.GetComponent(/datum/component/wet_floor)
	return !floor || floor.highest_strength <= TURF_WET_PERMAFROST

/proc/banda_atmos_environment(turf/target, active)
	var/turf/open/open = isopenturf(target) ? target : null
	if(!open?.air)
		open = null
	var/list/output = list(open ? (istype(open.air, /datum/gas_mixture/immutable/space) ? 1 : 0) : 2,
		open ? !!open.run_later : FALSE, open ? !!active : FALSE, open ? !!open.planetary_atmos : FALSE,
		target.temperature, target.temperature_archived || 0, target.heat_capacity, target.thermal_conductivity,
		target.atmos_supeconductivity, (target in SSair.active_super_conductivity), !!target.blocks_air)
	if(open?.planetary_atmos)
		var/datum/gas_mixture/planet = SSair.planetary[open.initial_gas_mix]
		if(!planet)
			CRASH("Native binding requires the cached planetary atmosphere")
		output += banda_atmos_air_packet(planet)
	return output

/proc/banda_atmos_turf_air(turf/target)
	if(isopenturf(target))
		var/turf/open/open = target
		if(open.air)
			return open.air
		if(!target.blocks_air)
			CRASH("Initialized open turf has no native atmosphere input")
	var/datum/gas_mixture/empty = new(CELL_VOLUME)
	empty.temperature = TCMB
	return empty

/proc/banda_atmos_terrain_packet(turf/target)
	return list(target.blocks_air ? 2 : 0, target.temperature, target.temperature_archived || 0,
		target.heat_capacity, target.thermal_conductivity, target.atmos_supeconductivity)
