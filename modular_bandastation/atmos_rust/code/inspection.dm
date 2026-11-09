/datum/banda_atmos_phase/world_adapter
	/// Additive DLL query; installations with older DLLs retain the original providers.
	var/inspection_queries_ready = FALSE

/// One detached numeric reading; no gas_mixture datum or archive is materialized.
/proc/banda_atmos_inspection_values(list/values, reference)
	if(!values)
		return null
	var/list/moles = list()
	var/list/gases = banda_atmos_gases()
	var/index = BANDA_AIR_GASES
	for(var/count in 1 to values[BANDA_AIR_GAS_COUNT])
		var/gas = gases[values[index++] + 1]
		moles[gas] = values[index++]
	var/result_count = values[index++]
	var/list/results = list()
	var/list/reactions = banda_atmos_reactions()
	for(var/count in 1 to result_count)
		var/reaction = reactions[values[index++] + 1]
		results[reaction] = values[index++]
	return list(
		"moles" = moles,
		"reaction_results" = results,
		"temperature" = values[BANDA_AIR_TEMPERATURE],
		"pressure" = values[BANDA_AIR_PRESSURE],
		"volume" = values[BANDA_AIR_VOLUME],
		"heat_capacity" = values[BANDA_AIR_HEAT_CAPACITY],
		"thermal_energy" = values[BANDA_AIR_HEAT_CAPACITY] * values[BANDA_AIR_TEMPERATURE],
		"total_moles" = values[BANDA_AIR_TOTAL_MOLES],
		"reference" = reference,
	)

/// Existing DM-owned samples stay DM-owned; reading a native port never exposes it.
/proc/banda_atmos_inspect_mixture(datum/gas_mixture/air)
	if(!air)
		return null
	var/datum/banda_atmos_pipe_binding/binding = air.banda_pipe_binding
	if(binding?.native_owned)
		var/datum/banda_atmos_phase/world_adapter/phase = istype(binding.owner, /datum/banda_atmos_phase/world_adapter) ? binding.owner : null
		if(phase?.inspection_queries_ready && !phase.closed)
			var/list/output = list()
			phase.native(91, list(1, binding.handle[1], binding.handle[2]), output)
			if(output[3] != 1 || output[4] != binding.handle[2])
				CRASH("Invalid native port inspection")
			return banda_atmos_inspection_values(output, REF(air))
		// Older DLLs need actual gas, rather than the empty VM shell of a native-owned port.
		binding.restore(air)
	return list(
		"moles" = air.moles.Copy(),
		"reaction_results" = air.reaction_results.Copy(),
		"temperature" = air.temperature,
		"pressure" = air.return_pressure(),
		"volume" = air.volume,
		"heat_capacity" = air.heat_capacity(),
		"thermal_energy" = air.thermal_energy(),
		"total_moles" = air.total_moles(),
		"reference" = REF(air),
	)

/// Fallback preserves custom providers, including their refresh/processing side effects.
/atom/proc/return_analyzable_readings()
	var/air = return_analyzable_air()
	if(!air)
		return null
	return banda_atmos_inspect_mixtures(islist(air) ? air : list(air))

/// Preserve repeated aliases so original Find() node numbering stays unchanged.
/proc/banda_atmos_inspect_mixtures(list/airs)
	var/list/output = list()
	var/list/seen = list()
	for(var/datum/gas_mixture/air as anything in airs)
		var/list/reading = seen[air]
		if(!reading)
			reading = banda_atmos_inspect_mixture(air)
			seen[air] = reading
		output += list(reading)
	return output

/turf/open/return_analyzable_readings()
	var/list/reading = return_air_reading()
	return reading ? list(reading) : null

/// Numeric counterpart of return_air(), retaining an existing live alias during migration.
/atom/proc/return_air_reading()
	return banda_atmos_inspect_mixture(return_air())

/turf/open/return_air_reading()
	var/datum/banda_atmos_phase/world_adapter/phase = SSair.native_atmos
	var/list/values = phase?.read_air_values(src, null, TRUE, TRUE)
	if(values)
		// Only a React key uses reference; binding replacement must invalidate that identity.
		return banda_atmos_inspection_values(values, "rust_air_[REF(src)]_[values[4]]")
	return ..()

/obj/machinery/atmospherics/components/return_analyzable_readings()
	return banda_atmos_inspect_mixtures(airs)

/obj/machinery/atmospherics/components/tank/return_analyzable_readings()
	return list(banda_atmos_inspect_mixture(air_contents))

/obj/machinery/air_sensor/return_air_reading()
	if(!on)
		return null
	. = loc?.return_air_reading()
	use_energy(active_power_usage)

/// Parser output remains identical to gas_mixture_parser; internal reading fields do not leak into UI.
/proc/gas_reading_parser(list/reading, name)
	var/list/output = list("gases" = list(), "reactions" = list(), "name" = format_text(name), "total_moles" = null, "temperature" = null, "volume" = null, "pressure" = null, "reference" = null)
	if(!reading)
		return output
	var/list/gases = reading["moles"]
	for(var/gas, amount in gases)
		output["gases"] += list(list(GAS_META[META_GAS_ID][gas], GAS_META[META_GAS_NAME][gas], amount))
	var/list/results = reading["reaction_results"]
	for(var/datum/gas_reaction/standard/reaction as anything in results)
		output["reactions"] += list(list(initial(reaction.id), initial(reaction.name), results[reaction]))
	for(var/key in list("total_moles", "temperature", "volume", "pressure", "reference"))
		output[key] = reading[key]
	return output
