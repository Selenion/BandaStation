//! Banda's active-cell gas/reaction pass and excited groups on an explicit 3D neighbour graph.
use crate::storage::Storage;
use crate::{
    catalog::*,
    cell::{Cell, WorldCommand},
    thermal::Terrain,
    transport::{Boundary, Sample, SampleView},
    Archive, Mixture, REACTING_FLAG, STOP_FLAG, VOLATILE_FLAG,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ops::{Deref, DerefMut},
    sync::Arc,
};

pub type Id = u32;

#[cfg(test)]
mod sharing_tests {
    use super::*;

    fn space(coordinates: [i32; 3]) -> Tile {
        let air = Mixture::new(TCMB, CELL_VOLUME, true);
        let mut tile = Tile::new(coordinates, &air, false);
        tile.boundary = Boundary::Space;
        tile.excited = false;
        tile
    }

    #[test]
    fn equal_space_state_keeps_coordinates_links_and_snapshot_writes_independent() {
        let mut schedule = Schedule::default();
        let first = space([1, 2, 3]);
        let mut second = space([2, 2, 3]);
        second.thermal_neighbors[0] = Some(7);
        schedule.add_tile(1, first);
        schedule.add_tile(2, second);
        assert!(Arc::ptr_eq(
            &schedule.tiles[&1].state,
            &schedule.tiles[&2].state
        ));
        let held = schedule.clone();
        let tile = schedule.tiles.get_mut(&2).unwrap();
        tile.group = Some(9);
        tile.ticker = 23;
        tile.terrain.temperature = 700.0;
        assert_eq!(schedule.tiles[&1].group, None);
        assert_eq!(schedule.tiles[&1].ticker, 0);
        assert_eq!(held.tiles[&2].terrain.temperature, 293.15);
        assert_eq!(schedule.tiles[&2].coordinates, [2, 2, 3]);
        assert_eq!(schedule.tiles[&2].thermal_neighbors[0], Some(7));
    }

    #[test]
    fn different_space_metadata_and_signed_zero_never_share() {
        let base = space([1, 1, 1]);
        let mut changed = base.clone();
        changed.pressure = -0.0;
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.terrain.conductivity = -0.0;
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.archived.temperature = -0.0;
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.group = Some(1);
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.ticker = 1;
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.has_air = false;
        assert!(!base.state.same_space_bits(&changed.state));
        changed = base.clone();
        changed.boundary = Boundary::Mutable;
        assert!(!changed.state.same_space_bits(&changed.state));
    }
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub coordinates: [i32; 3],
    pub thermal_neighbors: [Option<Id>; 4],
    state: Arc<TileState>,
}

/// Coordinate-independent data can be shared by equal space tiles and separated on first write.
#[derive(Clone, Debug, PartialEq)]
pub struct TileState {
    /// DM can temporarily block an open turf without deleting its gas mixture.
    pub has_air: bool,
    pub neighbors: Vec<Id>,
    pub archived: Archive,
    pub archived_cycle: u32,
    pub current_cycle: u32,
    pub ticker: u32,
    pub excited: bool,
    pub group: Option<u64>,
    // Space publishes the shared value through Schedule::last_share instead of copying every tile.
    pub(crate) last_share: f32,
    pub pressure: f32,
    pub direction: u8,
    pub thermal_enabled: bool,
    pub boundary: Boundary,
    pub planetary: Option<Box<Sample>>,
    pub run_later: bool,
    pub terrain: Terrain,
}

impl Tile {
    pub fn new(coordinates: [i32; 3], air: &Mixture, thermal_enabled: bool) -> Self {
        Self {
            coordinates,
            thermal_neighbors: [None; 4],
            state: Arc::new(TileState {
                has_air: true,
                neighbors: Vec::new(),
                archived: Archive::empty(air.turf_heat_capacity),
                archived_cycle: 0,
                current_cycle: 0,
                ticker: 0,
                excited: true,
                group: None,
                last_share: 0.0,
                pressure: 0.0,
                direction: 0,
                thermal_enabled,
                boundary: Boundary::Mutable,
                planetary: None,
                run_later: false,
                terrain: Terrain::new(thermal_enabled),
            }),
        }
    }
}

impl Deref for Tile {
    type Target = TileState;

    fn deref(&self) -> &TileState {
        &self.state
    }
}

impl DerefMut for Tile {
    fn deref_mut(&mut self) -> &mut TileState {
        Arc::make_mut(&mut self.state)
    }
}

impl TileState {
    fn same_space_bits(&self, other: &Self) -> bool {
        matches!(self.boundary, Boundary::Space)
            && matches!(other.boundary, Boundary::Space)
            && self.planetary.is_none()
            && other.planetary.is_none()
            && self == other
            && self.archived.same_bits(&other.archived)
            && self.last_share.to_bits() == other.last_share.to_bits()
            && self.pressure.to_bits() == other.pressure.to_bits()
            && self.terrain.temperature.to_bits() == other.terrain.temperature.to_bits()
            && self.terrain.archived.to_bits() == other.terrain.archived.to_bits()
            && self.terrain.capacity.to_bits() == other.terrain.capacity.to_bits()
            && self.terrain.conductivity.to_bits() == other.terrain.conductivity.to_bits()
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub members: Vec<Id>,
    pub breakdown: u32,
    pub dismantle: u32,
    pub reactions: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Schedule {
    pub tiles: Storage<Tile>,
    pub active: Vec<Id>,
    pub groups: BTreeMap<u64, Group>,
    next_group: u64,
    space_last_share: f32,
    pub hotspots: Vec<Id>,
    pub conducting: Vec<Id>,
    pressure_queue: Vec<Id>,
    // Membership is separate so wind commands keep DM's insertion order.
    pressure_queued: HashSet<Id>,
    /// Present only for DM builds with TRACK_MAX_SHARE, not part of normal simulation storage.
    pub max_shares: Option<BTreeMap<Id, f32>>,
    /// Conservative set of cells whose observable state this calculation may change.
    pub(crate) touched: BTreeSet<Id>,
    pub(crate) space_overrides: BTreeSet<Id>,
}

impl Schedule {
    /// Copy the small scheduler state; physical tile records are synchronized separately.
    pub(crate) fn copy_queues(&mut self, source: &Self) {
        self.active.clone_from(&source.active);
        self.groups.clone_from(&source.groups);
        self.next_group = source.next_group;
        self.space_overrides.clone_from(&source.space_overrides);
        self.space_last_share = source.space_last_share;
        self.hotspots.clone_from(&source.hotspots);
        self.conducting.clone_from(&source.conducting);
        self.pressure_queue.clone_from(&source.pressure_queue);
        self.pressure_queued.clone_from(&source.pressure_queued);
        self.max_shares.clone_from(&source.max_shares);
        self.touched.clear();
    }

    pub fn add(&mut self, id: Id, coordinates: [i32; 3], air: &Mixture, thermal_enabled: bool) {
        self.add_tile(id, Tile::new(coordinates, air, thermal_enabled));
    }

    pub fn add_tile(&mut self, id: Id, tile: Tile) {
        if tile.pressure != 0.0 {
            self.pressure_queue.push(id);
            self.pressure_queued.insert(id);
        }
        if tile.excited && tile.has_air {
            self.active.push(id);
        }
        self.store_tile(id, tile);
        self.refresh_space_override(id);
    }

    pub(crate) fn store_tile(&mut self, id: Id, mut tile: Tile) {
        if let Some(previous) = id.checked_sub(1).and_then(|id| self.tiles.get(&id)) {
            if tile.state.same_space_bits(&previous.state) {
                tile.state = previous.state.clone();
            }
        }
        self.tiles.insert(id, tile);
    }

    pub(crate) fn refresh_space_override(&mut self, id: Id) {
        let tile = &self.tiles[&id];
        if matches!(tile.boundary, Boundary::Space)
            && tile.last_share.to_bits() != self.space_last_share.to_bits()
        {
            self.space_overrides.insert(id);
        } else {
            self.space_overrides.remove(&id);
        }
    }

    /// New/replaced mixtures keep their local value until the next atmosphere pass publishes it.
    pub fn last_share(&self, id: Id) -> f32 {
        let tile = &self.tiles[&id];
        if matches!(tile.boundary, Boundary::Space) && !self.space_overrides.contains(&id) {
            self.space_last_share
        } else {
            tile.last_share
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let mut positions = BTreeSet::new();
        for tile in self.tiles.values() {
            if !positions.insert(tile.coordinates) {
                return Err("Duplicate tile coordinates");
            }
        }
        self.validate_tiles(self.tiles.keys())
    }

    pub(crate) fn validate_tiles(&self, ids: impl Iterator<Item = Id>) -> Result<(), &'static str> {
        for id in ids {
            let Some(tile) = self.tiles.get(&id) else {
                continue;
            };
            let mut seen = BTreeSet::new();
            for neighbor in &tile.neighbors {
                let other = self.tiles.get(neighbor).ok_or("Missing neighbor")?;
                if !seen.insert(*neighbor) || *neighbor == id {
                    return Err("Duplicate/self neighbor");
                }
                let distance: i64 = tile
                    .coordinates
                    .iter()
                    .zip(other.coordinates)
                    .map(|(a, b)| (i64::from(*a) - i64::from(b)).abs())
                    .sum();
                if distance != 1 || !other.neighbors.contains(&id) {
                    return Err("Non-cardinal or asymmetric connection");
                }
            }
            for (index, neighbor) in tile.thermal_neighbors.iter().enumerate() {
                if let Some(neighbor) = neighbor {
                    let other = self.tiles.get(neighbor).ok_or("Missing thermal neighbor")?;
                    let delta = [[0, 1, 0], [0, -1, 0], [1, 0, 0], [-1, 0, 0]][index];
                    if other.coordinates != std::array::from_fn(|i| tile.coordinates[i] + delta[i])
                    {
                        return Err("Incorrect thermal direction");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn activate(&mut self, id: Id) {
        if let Some(tile) = self.tiles.get_mut(&id) {
            if !tile.has_air {
                return;
            }
            tile.ticker = 0;
            if !tile.excited {
                tile.excited = true;
                self.active.push(id);
            }
        }
    }

    pub fn remove(&mut self, id: Id) -> Vec<Id> {
        let mut affected = vec![id];
        if let Some(shares) = &mut self.max_shares {
            shares.remove(&id);
        }
        if let Some(group) = self.tiles.get(&id).and_then(|t| t.group) {
            affected.extend(self.groups[&group].members.iter().copied());
            self.clear_group(group);
        }
        self.active.retain(|v| *v != id);
        self.tiles.remove(&id);
        self.hotspots.retain(|v| *v != id);
        self.conducting.retain(|v| *v != id);
        self.pressure_queue.retain(|v| *v != id);
        self.pressure_queued.remove(&id);
        let linked: Vec<_> = self
            .tiles
            .iter()
            .filter_map(|(other, tile)| {
                (tile.neighbors.contains(&id) || tile.thermal_neighbors.contains(&Some(id)))
                    .then_some(other)
            })
            .collect();
        for other in linked {
            affected.push(other);
            let tile = self.tiles.get_mut(&other).unwrap();
            tile.neighbors.retain(|v| *v != id);
            for neighbor in &mut tile.thermal_neighbors {
                if *neighbor == Some(id) {
                    *neighbor = None;
                }
            }
        }
        self.space_overrides.remove(&id);
        affected
    }

    pub fn run(&mut self, cells: &mut Storage<Cell>, cycle: u32) -> Vec<(Id, WorldCommand)> {
        self.run_stages(cells, cycle, false)
    }

    pub fn tick(&mut self, cells: &mut Storage<Cell>, cycle: u32) -> Vec<(Id, WorldCommand)> {
        self.run_stages(cells, cycle, true)
    }

    fn run_stages(
        &mut self,
        cells: &mut Storage<Cell>,
        cycle: u32,
        fire: bool,
    ) -> Vec<(Id, WorldCommand)> {
        let mut commands = Vec::new();
        self.touched.clear();
        for id in self.pressure_queue.drain(..) {
            self.touched.insert(id);
            self.tiles.get_mut(&id).unwrap().pressure = 0.0;
        }
        self.pressure_queued.clear();
        // DM copies active_turfs and pops from its end; new activations wait until next pass.
        let current = self.active.clone();
        let mut sleeping = false;
        for &id in current.iter().rev() {
            self.touched.insert(id);
            self.touched
                .extend(self.tiles[&id].neighbors.iter().copied());
            let had_spot = cells[&id].hotspot.is_some();
            sleeping |= self.process_cell(cells, id, cycle);
            if !had_spot && cells[&id].hotspot.is_some() {
                self.hotspots.push(id);
                // The original constructor clears the protection when born during its cell's pass.
                cells
                    .get_mut(&id)
                    .unwrap()
                    .hotspot
                    .as_mut()
                    .unwrap()
                    .just_spawned = false;
            }
            commands.extend(
                cells
                    .get_mut(&id)
                    .unwrap()
                    .commands
                    .drain(..)
                    .map(|effect| (id, effect)),
            );
        }
        // Processed neighbors cannot be reactivated by this pass (current_cycle guards them).
        // Compact once after the fixed snapshot, keeping survivors and new activations in order.
        if sleeping {
            self.active.retain(|id| self.tiles[id].excited);
        }
        if fire {
            for id in self.hotspots.clone().into_iter().rev() {
                self.touched.insert(id);
                self.touched
                    .extend(self.tiles[&id].neighbors.iter().copied());
                let cell = cells.get_mut(&id).unwrap();
                let protected = cell.hotspot.as_ref().is_some_and(|spot| spot.just_spawned);
                if !protected {
                    if let Some(group) = self.tiles[&id].group {
                        let group = self.groups.get_mut(&group).unwrap();
                        group.breakdown = 0;
                        group.dismantle = 0;
                    }
                }
                if cell.process_hotspot() {
                    // perform_exposure returns the sample through assume_air, which reactivates its turf.
                    self.activate(id);
                }
                commands.extend(cell.commands.drain(..).map(|effect| (id, effect)));
                if let Some(spot) = &cell.hotspot {
                    if spot.bypassing
                        && (cell.air.temperature > FIRE_MINIMUM_TEMPERATURE_TO_SPREAD
                            || spot.cold_fire)
                    {
                        let temperature = cell.air.temperature
                            * if spot.cold_fire {
                                COLD_FIRE_SPREAD_RADIOSITY_SCALE
                            } else {
                                FIRE_SPREAD_RADIOSITY_SCALE
                            };
                        for neighbor in self.tiles[&id].neighbors.clone() {
                            let other = cells.get_mut(&neighbor).unwrap();
                            if other.hotspot.is_some() || self.tiles[&neighbor].boundary.immutable()
                            {
                                continue;
                            }
                            other.hotspot_expose(temperature, CELL_VOLUME / 4.0, false);
                            if let Some(spot) = other.hotspot.as_mut() {
                                spot.just_spawned = self.tiles[&neighbor].current_cycle < cycle;
                                self.hotspots.push(neighbor);
                                self.activate(neighbor);
                            }
                            commands
                                .extend(other.commands.drain(..).map(|effect| (neighbor, effect)));
                        }
                    }
                } else {
                    self.hotspots.retain(|v| *v != id);
                }
            }
        }
        for group in self.groups.values() {
            self.touched.extend(group.members.iter().copied());
        }
        let groups: Vec<_> = self.groups.keys().copied().collect();
        for id in groups.into_iter().rev() {
            let Some(group) = self.groups.get_mut(&id) else {
                continue;
            };
            let volatile = group.reactions & VOLATILE_FLAG != 0;
            group.breakdown += 1;
            if !volatile {
                group.dismantle += 1;
            }
            if group.breakdown >= EXCITED_GROUP_BREAKDOWN_CYCLES as u32 && !volatile {
                self.breakdown(cells, id);
            } else if group.dismantle >= EXCITED_GROUP_DISMANTLE_CYCLES as u32
                && group.reactions & (REACTING_FLAG | STOP_FLAG) == 0
            {
                let members = group.members.clone();
                for &member in &members {
                    let tile = self.tiles.get_mut(&member).unwrap();
                    tile.excited = false;
                    tile.ticker = 0;
                }
                let members: HashSet<_> = members.into_iter().collect();
                self.active.retain(|member| !members.contains(member));
                self.clear_group(id);
            }
            if let Some(group) = self.groups.get_mut(&id) {
                group.reactions = 0;
            }
        }
        if fire {
            for &id in self.pressure_queue.iter().rev() {
                let tile = &self.tiles[&id];
                commands.push((
                    id,
                    WorldCommand {
                        action: crate::cell::WorldAction::Pressure {
                            difference: tile.pressure,
                            direction: tile.direction,
                        },
                        air: cells[&id].air.clone(),
                    },
                ));
            }
            commands.extend(self.conduct(cells, cycle));
        }
        // Banda's space mixture is shared. Publish one scalar instead of writing every space tile.
        self.space_overrides.clear();
        commands
    }

    pub(crate) fn archive(&mut self, cells: &Storage<Cell>, id: Id, cycle: u32) {
        let tile = self.tiles.get_mut(&id).unwrap();
        tile.terrain.archived = tile.terrain.temperature;
        // Base turf.archive does not advance archived_cycle; closed neighbors may be rearchived.
        if !tile.has_air {
            return;
        }
        if !tile.boundary.immutable() {
            tile.archived.copy_from(&cells[&id].air);
        }
        tile.archived_cycle = cycle;
    }

    fn process_cell(&mut self, cells: &mut Storage<Cell>, id: Id, cycle: u32) -> bool {
        if let Some(shares) = &mut self.max_shares {
            shares.insert(id, 0.0);
        }
        if self.tiles[&id].archived_cycle < cycle {
            self.archive(cells, id, cycle);
        }
        self.tiles.get_mut(&id).unwrap().current_cycle = cycle;
        let mut ticker = self.tiles[&id].ticker + 1;
        let neighbors = self.tiles[&id].neighbors.clone();
        let own_coefficient = 1.0 / (neighbors.len() + 1) as f32;
        let mut share_end = Vec::new();
        for neighbor in neighbors {
            if self.tiles[&neighbor].run_later {
                share_end.push(neighbor);
            }
            if cycle <= self.tiles[&neighbor].current_cycle {
                continue;
            }
            // LINDA_CYCLE_ARCHIVE is unconditional here, including an already archived neighbor.
            self.archive(cells, neighbor, cycle);
            let mut own_group = self.tiles[&id].group;
            let other_group = self.tiles[&neighbor].group;
            if let (Some(own), Some(other)) = (own_group, other_group) {
                if own != other {
                    self.merge_groups(own, other);
                    own_group = self.tiles[&id].group;
                }
            }
            let share =
                if own_group.is_some() && other_group.is_some() && self.tiles[&neighbor].excited {
                    true
                } else if different(
                    &cells[&id].air,
                    &self.tiles[&id].archived,
                    &cells[&neighbor].air,
                    &self.tiles[&neighbor].archived,
                ) {
                    self.activate(neighbor);
                    let group = own_group
                        .or(self.tiles[&neighbor].group)
                        .unwrap_or_else(|| self.new_group());
                    if own_group.is_none() {
                        self.add_member(group, id);
                    }
                    if self.tiles[&neighbor].group.is_none() {
                        self.add_member(group, neighbor);
                    }
                    true
                } else {
                    false
                };
            if !share {
                continue;
            }
            let other_coefficient = 1.0 / (self.tiles[&neighbor].neighbors.len() + 1) as f32;
            let difference = self.exchange(cells, id, neighbor, own_coefficient, other_coefficient);
            let moved = self.tiles[&id].last_share;
            if let Some(shares) = &mut self.max_shares {
                let maximum = shares.entry(id).or_default();
                *maximum = maximum.max(moved);
            }
            if let Some(pressure) = difference {
                if pressure > 0.0 {
                    self.pressure(id, neighbor, pressure);
                } else if pressure < 0.0 {
                    self.pressure(neighbor, id, -pressure);
                }
            }
            let group = self.tiles[&id].group.unwrap();
            if moved > MINIMUM_AIR_TO_SUSPEND {
                let group = self.groups.get_mut(&group).unwrap();
                group.breakdown = 0;
                group.dismantle = 0;
                ticker = 0;
                self.tiles.get_mut(&neighbor).unwrap().ticker = 0;
            } else if moved > MINIMUM_MOLES_DELTA_TO_MOVE {
                self.groups.get_mut(&group).unwrap().dismantle = 0;
                ticker = 0;
                self.tiles.get_mut(&neighbor).unwrap().ticker = 0;
            }
        }
        if self.tiles[&id].planetary.is_some() {
            self.archive(cells, id, cycle);
            let different = {
                let planet = self.tiles[&id].planetary.as_ref().unwrap();
                different(
                    &cells[&id].air,
                    &self.tiles[&id].archived,
                    &planet.air,
                    &planet.archived,
                )
            };
            if different {
                if self.tiles[&id].group.is_none() {
                    let g = self.new_group();
                    self.add_member(g, id);
                }
                let tile: &mut TileState = self.tiles.get_mut(&id).unwrap();
                let planet = tile.planetary.as_mut().unwrap();
                let mut own = SampleView {
                    air: &mut cells.get_mut(&id).unwrap().air,
                    archived: &mut tile.archived,
                    last_share: &mut tile.last_share,
                    boundary: &tile.boundary,
                };
                let mut atmosphere = planet.view();
                own.share(&mut atmosphere, 0.8, 0.8);
                let capacity = atmosphere.air.heat_capacity() * 5.0;
                own.temperature_share_capacity(
                    &mut atmosphere,
                    OPEN_HEAT_TRANSFER_COEFFICIENT,
                    capacity,
                );
                atmosphere.garbage_collect();
                if let Some(shares) = &mut self.max_shares {
                    let maximum = shares.entry(id).or_default();
                    *maximum = maximum.max(tile.last_share);
                }
                let group = self.groups.get_mut(&tile.group.unwrap()).unwrap();
                if tile.last_share > MINIMUM_AIR_TO_SUSPEND {
                    group.breakdown = 0;
                    group.dismantle = 0;
                    ticker = 0;
                } else if tile.last_share > MINIMUM_MOLES_DELTA_TO_MOVE {
                    group.dismantle = 0;
                    ticker = 0;
                }
            }
        }
        for neighbor in share_end {
            self.archive(cells, id, cycle);
            let difference = self.exchange(cells, id, neighbor, 1.0, 1.0);
            let moved = self.tiles[&id].last_share;
            if let Some(shares) = &mut self.max_shares {
                let maximum = shares.entry(id).or_default();
                *maximum = maximum.max(moved);
            }
            if moved > MINIMUM_MOLES_DELTA_TO_MOVE {
                // run_later can move gas even when compare() did not create a group.
                // Keep the exchange/tickers, but do not create a group just for its timers.
                if let Some(group_id) = self.tiles[&id].group {
                    let group = self.groups.get_mut(&group_id).unwrap();
                    if moved > MINIMUM_AIR_TO_SUSPEND {
                        group.breakdown = 0;
                    }
                    group.dismantle = 0;
                }
                ticker = 0;
                self.tiles.get_mut(&neighbor).unwrap().ticker = 0;
            }
            if let Some(p) = difference {
                if p > 0.0 {
                    self.pressure(id, neighbor, p);
                }
                // Original run_later passes a negative value here, unlike the ordinary exchange.
                else if p < 0.0 {
                    self.pressure(neighbor, id, p);
                }
            }
        }
        let flags = if self.tiles[&id].boundary.immutable() {
            0
        } else {
            cells.get_mut(&id).unwrap().react().flags
        };
        if let Some(group) = self.tiles[&id].group {
            self.groups.get_mut(&group).unwrap().reactions |= flags;
        }
        let tile = &self.tiles[&id];
        let cell = &cells[&id];
        let hot = tile.thermal_enabled
            && cell.air.temperature >= MINIMUM_TEMPERATURE_START_SUPERCONDUCTION
            && tile.boundary.heat_capacity(&cell.air) >= M_CELL_WITH_RATIO;
        if hot && !self.conducting.contains(&id) {
            self.conducting.push(id);
        }
        let tile = &self.tiles[&id];
        let sleeping = !hot
            && cell.hotspot.is_none()
            && flags & (REACTING_FLAG | STOP_FLAG) == 0
            && (tile.group.is_none() || ticker > EXCITED_GROUP_DISMANTLE_CYCLES as u32);
        if sleeping {
            self.tiles.get_mut(&id).unwrap().excited = false;
        }
        self.tiles.get_mut(&id).unwrap().ticker = ticker;
        let cell = cells.get_mut(&id).unwrap();
        cell.commands.push(WorldCommand {
            action: crate::cell::WorldAction::TurfExposure {
                temperature: cell.air.temperature,
                gas: true,
            },
            air: cell.air.clone(),
        });
        sleeping
    }

    fn exchange(
        &mut self,
        cells: &mut Storage<Cell>,
        id: Id,
        neighbor: Id,
        own_coeff: f32,
        other_coeff: f32,
    ) -> Option<f32> {
        let (own, other) = pair_mut(cells, id, neighbor);
        let (own_tile, other_tile) = pair_mut(&mut self.tiles, id, neighbor);
        let own_tile: &mut TileState = own_tile;
        let other_tile: &mut TileState = other_tile;
        let difference = SampleView {
            air: &mut own.air,
            archived: &mut own_tile.archived,
            last_share: &mut own_tile.last_share,
            boundary: &own_tile.boundary,
        }
        .share(
            &mut SampleView {
                air: &mut other.air,
                archived: &mut other_tile.archived,
                last_share: &mut other_tile.last_share,
                boundary: &other_tile.boundary,
            },
            own_coeff,
            other_coeff,
        );
        if matches!(own_tile.boundary, Boundary::Space) {
            self.space_last_share = own_tile.last_share;
        }
        difference
    }

    fn pressure(&mut self, from: Id, to: Id, pressure: f32) {
        if self.pressure_queued.insert(from) {
            self.pressure_queue.push(from);
        }
        let position = self.tiles[&to].coordinates;
        let tile = self.tiles.get_mut(&from).unwrap();
        if pressure > tile.pressure {
            tile.pressure = pressure;
            use std::cmp::Ordering::*;
            tile.direction = match position[0].cmp(&tile.coordinates[0]) {
                Greater => 4,
                Less => 8,
                Equal => 0,
            } | match position[1].cmp(&tile.coordinates[1]) {
                Greater => 1,
                Less => 2,
                Equal => 0,
            };
        }
    }

    fn new_group(&mut self) -> u64 {
        self.next_group += 1;
        self.groups.insert(
            self.next_group,
            Group {
                members: Vec::new(),
                breakdown: 0,
                dismantle: 0,
                reactions: 0,
            },
        );
        self.next_group
    }
    fn add_member(&mut self, group: u64, id: Id) {
        let entry = self.groups.get_mut(&group).unwrap();
        entry.members.push(id);
        entry.dismantle = 0;
        self.tiles.get_mut(&id).unwrap().group = Some(group);
    }

    fn merge_groups(&mut self, own: u64, other: u64) {
        let (keep, drop) = if self.groups[&own].members.len() > self.groups[&other].members.len() {
            (own, other)
        } else {
            (other, own)
        };
        let removed = self.groups.remove(&drop).unwrap();
        let kept = self.groups.get_mut(&keep).unwrap();
        kept.breakdown = kept.breakdown.min(removed.breakdown);
        kept.dismantle = 0;
        for member in removed.members {
            self.tiles.get_mut(&member).unwrap().group = Some(keep);
            kept.members.push(member);
        }
    }

    pub(crate) fn clear_group(&mut self, id: u64) {
        if let Some(group) = self.groups.remove(&id) {
            for member in group.members {
                self.tiles.get_mut(&member).unwrap().group = None;
            }
        }
    }

    fn breakdown(&mut self, cells: &mut Storage<Cell>, id: u64) {
        let members = self.groups[&id].members.clone();
        let mut shared = Mixture::new(TCMB, CELL_VOLUME, false);
        let (mut energy, mut capacity) = (0.0, 0.0);
        for member in &members {
            let air = &cells[member].air;
            let heat = self.tiles[member].boundary.heat_capacity(air);
            energy += air.temperature * heat;
            capacity += heat;
            for &gas in air.keys() {
                shared.add(gas, air.moles[gas]);
            }
        }
        shared.temperature = energy / capacity;
        for amount in shared.moles.iter_mut() {
            *amount /= members.len() as f32;
        }
        shared.garbage_collect();
        for member in members {
            if !self.tiles[&member].boundary.immutable() {
                let mixture = self.tiles[&member]
                    .planetary
                    .as_ref()
                    .map_or(&shared, |p| &p.air);
                let air = &mut cells.get_mut(&member).unwrap().air;
                air.moles.clone_from(&mixture.moles);
                air.order = mixture.order.clone();
                air.temperature = mixture.temperature;
                let keys = air.order.clone();
                let archived = &mut self.tiles.get_mut(&member).unwrap().archived;
                archived.moles.fill(0.0);
                archived.order = keys;
            }
            self.activate(member);
            self.tiles.get_mut(&member).unwrap().ticker = EXCITED_GROUP_DISMANTLE_CYCLES as u32;
        }
        self.groups.get_mut(&id).unwrap().breakdown = 0;
    }
}

pub(crate) fn pair_mut<T: Clone>(map: &mut Storage<T>, a: Id, b: Id) -> (&mut T, &mut T) {
    map.pair_mut(a, b)
}

fn different(
    own: &Mixture,
    own_archive: &Archive,
    other: &Mixture,
    other_archive: &Archive,
) -> bool {
    if (own.total_moles() - other.total_moles()).abs() > MINIMUM_MOLES_DELTA_TO_MOVE {
        return true;
    }
    for gas in own_archive.keys().iter().chain(other_archive.keys()) {
        let delta = (own_archive.moles[*gas] - other_archive.moles[*gas]).abs();
        if delta > MINIMUM_MOLES_DELTA_TO_MOVE
            && delta > own_archive.moles[*gas] * MINIMUM_AIR_RATIO_TO_MOVE
        {
            return true;
        }
    }
    own_archive.total_moles() > MINIMUM_MOLES_DELTA_TO_MOVE
        && (own_archive.temperature - other_archive.temperature).abs()
            > MINIMUM_TEMPERATURE_DELTA_TO_SUSPEND
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn quiet_tiles() -> (Schedule, Storage<Cell>) {
        let mut schedule = Schedule::default();
        let mut cells = Storage::new();
        for id in 1..=5 {
            let mut air = Mixture::new(293.15, CELL_VOLUME, true);
            air.set(NITROGEN, 100.0);
            schedule.add(id, [id as i32, 1, 1], &air, false);
            cells.insert(id, Cell::new(air, crate::Context::default()));
        }
        (schedule, cells)
    }

    #[test]
    fn pressure_order_includes_negative_and_zero_events_and_survives_copy_and_remove() {
        let (mut source, mut cells) = quiet_tiles();
        source.pressure(3, 2, -1.0);
        source.pressure(1, 2, 0.0);
        source.pressure(3, 2, 10.0);
        source.pressure(2, 1, 5.0);
        assert_eq!(source.pressure_queue, [3, 1, 2]);
        let mut copy = source.clone();
        copy.pressure(4, 5, 9.0);
        copy.copy_queues(&source);
        copy.pressure(3, 2, 20.0);
        assert_eq!(copy.pressure_queue, [3, 1, 2]);
        copy.remove(1);
        assert!(!copy.pressure_queued.contains(&1));
        assert_eq!(copy.pressure_queue, [3, 2]);
        source.run(&mut cells, 1);
        source.pressure(3, 2, -2.0);
        assert_eq!(source.pressure_queue, [3]);
        assert_eq!(source.pressure_queued.len(), 1);
        assert_eq!(copy.pressure_queue, [3, 2]);
    }

    #[test]
    fn sleeping_and_reactivation_keep_insertion_order() {
        let (mut schedule, mut cells) = quiet_tiles();
        schedule.active = vec![4, 2, 5, 1, 3];
        for id in [2, 5] {
            cells.get_mut(&id).unwrap().hotspot = Some(crate::cell::Hotspot {
                temperature: 700.0,
                volume: 100.0,
                just_spawned: false,
                bypassing: false,
                cold_fire: false,
            });
        }
        schedule.run(&mut cells, 1);
        assert_eq!(schedule.active, [2, 5]);
        schedule.activate(3);
        schedule.activate(4);
        schedule.activate(3);
        assert_eq!(schedule.active, [2, 5, 3, 4]);
    }

    #[test]
    fn dismantling_group_preserves_survivor_order() {
        let (mut schedule, mut cells) = quiet_tiles();
        schedule.active = vec![4, 2, 5, 1, 3];
        for id in [4, 5] {
            cells.get_mut(&id).unwrap().hotspot = Some(crate::cell::Hotspot {
                temperature: 700.0,
                volume: 100.0,
                just_spawned: false,
                bypassing: false,
                cold_fire: false,
            });
        }
        let group = schedule.new_group();
        for id in [3, 1, 2] {
            schedule.add_member(group, id);
        }
        schedule.groups.get_mut(&group).unwrap().dismantle =
            EXCITED_GROUP_DISMANTLE_CYCLES as u32 - 1;
        schedule.run(&mut cells, 1);
        assert_eq!(schedule.active, [4, 5]);
        assert!(schedule.groups.is_empty());
        for id in [1, 2, 3] {
            assert!(!schedule.tiles[&id].excited);
            assert_eq!(schedule.tiles[&id].ticker, 0);
        }
    }
}
