/// DM owns light, sound, smoothing and contact burns; Rust owns the gas lifecycle.
/obj/effect/hotspot
	var/datum/banda_atmos_cell/binding

/obj/effect/hotspot/banda_native/Initialize(mapload, starting_volume, starting_temperature, datum/banda_atmos_cell/cell)
	binding = cell
	. = ..()
	if(!cell.owner.world_owned)
		SSair.hotspots -= src

/obj/effect/hotspot/banda_native/perform_exposure()
	if(!binding)
		return ..()
	var/turf/open/open = loc
	open.active_hotspot = src
	return TRUE

/obj/effect/hotspot/banda_native/process()
	if(!binding)
		return ..()
	update_native_presentation()

/obj/effect/hotspot/proc/update_native_presentation()
	set_fire_stage(bypassing ? "heavy" : (volume > CELL_VOLUME * 0.4 ? "medium" : "light"))
	update_color()
	var/mutable_appearance/heat_overlay = mutable_appearance('modular_bandastation/aesthetics/fire/icons/fire.dmi', fire_stage)
	heat_overlay.blend_mode = BLEND_ADD
	add_overlay(heat_overlay)

/datum/banda_atmos_phase/world_adapter/proc/import_hotspot(obj/effect/hotspot/spot)
	var/datum/banda_atmos_cell/cell = spot.binding
	native(29, list(cell.native_id, spot.temperature, spot.volume, !!spot.just_spawned, !!spot.bypassing, !!spot.cold_fire))

/// Direct hotspot constructors remain valid world actors; subsequent numeric ticks are native.
/datum/banda_atmos_phase/world_adapter/proc/adopt_hotspot(obj/effect/hotspot/spot)
	if(QDELETED(spot) || !isopenturf(spot.loc))
		return
	var/turf/open/open = spot.loc
	var/datum/banda_atmos_cell/cell = ensure_binding(open)
	if(!cell)
		return
	if(cell.pending_rebind || cell.updating_hotspot)
		cell.hotspots_pending = TRUE
		return
	spot.binding = cell
	if(open.air)
		cell.commit_air(open.air, FALSE, FALSE)
	cell.cache_current = FALSE
	import_hotspot(spot)

/// Only a constructor rejected by a temporary binding lock needs this local reconciliation.
/datum/banda_atmos_cell/proc/adopt_pending_hotspots()
	SHOULD_NOT_SLEEP(TRUE)
	if(pending_rebind || updating_hotspot || !owner?.world_owned || owner.closed || !isopenturf(target))
		return
	var/turf/open/open = target
	if(!(open.flags_1 & INITIALIZED_1))
		return
	var/version = binding_version
	var/datum/banda_atmos_phase/world_adapter/phase = owner
	for(var/obj/effect/hotspot/spot in open)
		if(binding_version != version || pending_rebind || open.banda_atmos_cell != src)
			return
		if(QDELETED(spot) || spot.binding)
			continue
		// Turf destruction can overwrite the pointer after a deletion callback creates its successor.
		open.active_hotspot = spot
		phase.adopt_hotspot(spot)
	hotspots_pending = FALSE
