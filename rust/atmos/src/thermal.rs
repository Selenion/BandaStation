//! Original tg/Banda superconduction order, including solid and gas archives.
use crate::storage::Storage;
use crate::{
    catalog::*,
    cell::{Cell, WorldAction, WorldCommand},
    network::{pair_mut, Id, Schedule, TileState},
    transport::SampleView,
};

pub const CARDINALS: [u8; 4] = [1, 2, 4, 8];

#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    pub closed: bool,
    pub temperature: f32,
    pub archived: f32,
    pub capacity: f32,
    pub conductivity: f32,
    pub blocked: u8,
}
impl Terrain {
    pub fn new(enabled: bool) -> Self {
        Self {
            closed: false,
            temperature: 293.15,
            archived: TCMB,
            capacity: 500000.0,
            conductivity: if enabled {
                OPEN_HEAT_TRANSFER_COEFFICIENT
            } else {
                0.0
            },
            blocked: 0,
        }
    }
}

impl Schedule {
    pub fn consider_conduction(&mut self, cells: &Storage<Cell>, id: Id, starting: bool) -> bool {
        let tile = &self.tiles[&id];
        let threshold = if starting {
            MINIMUM_TEMPERATURE_START_SUPERCONDUCTION
        } else {
            MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION
        };
        let temperature = if tile.terrain.closed {
            tile.terrain.temperature
        } else {
            cells[&id].air.temperature
        };
        if temperature < threshold || tile.terrain.conductivity == 0.0 {
            return false;
        }
        if !tile.terrain.closed && tile.boundary.heat_capacity(&cells[&id].air) < M_CELL_WITH_RATIO
        {
            return false;
        }
        if !self.conducting.contains(&id) {
            self.conducting.push(id);
        }
        true
    }

    pub(crate) fn conduct(
        &mut self,
        cells: &mut Storage<Cell>,
        cycle: u32,
    ) -> Vec<(Id, WorldCommand)> {
        let mut commands = Vec::new();
        for id in self.conducting.clone().into_iter().rev() {
            self.touched.insert(id);
            self.touched
                .extend(self.tiles[&id].thermal_neighbors.iter().flatten().copied());
            if self.tiles[&id].terrain.closed && self.tiles[&id].archived_cycle < cycle {
                self.archive(cells, id, cycle);
            }
            let links = self.tiles[&id].thermal_neighbors;
            for (index, neighbor) in links.into_iter().enumerate() {
                let Some(neighbor) = neighbor else {
                    continue;
                };
                let tile = &self.tiles[&id];
                if !tile.terrain.closed
                    && (tile.neighbors.contains(&neighbor)
                        || tile.terrain.blocked & CARDINALS[index] != 0)
                {
                    continue;
                }
                if self.tiles[&neighbor].terrain.conductivity == 0.0 {
                    continue;
                }
                if self.tiles[&neighbor].archived_cycle < cycle {
                    self.archive(cells, neighbor, cycle);
                }
                let (own, other) = pair_mut(cells, id, neighbor);
                let (tile, other_tile) = pair_mut(&mut self.tiles, id, neighbor);
                let tile: &mut TileState = tile;
                let other_tile: &mut TileState = other_tile;
                if tile.terrain.closed && other_tile.terrain.closed {
                    let delta = other_tile.terrain.archived - tile.terrain.archived;
                    if delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
                        && tile.terrain.capacity != 0.0
                        && other_tile.terrain.capacity != 0.0
                    {
                        let heat = other_tile.terrain.conductivity
                            * (delta
                                * (tile.terrain.capacity
                                    * (other_tile.terrain.capacity
                                        / (tile.terrain.capacity + other_tile.terrain.capacity))));
                        tile.terrain.temperature += heat / tile.terrain.capacity;
                        other_tile.terrain.temperature -= heat / other_tile.terrain.capacity;
                    }
                } else if tile.terrain.closed {
                    tile.terrain.temperature = SampleView {
                        air: &mut other.air,
                        archived: &mut other_tile.archived,
                        last_share: &mut other_tile.last_share,
                        boundary: &other_tile.boundary,
                    }
                    .temperature_share_solid(
                        tile.terrain.conductivity,
                        tile.terrain.temperature,
                        tile.terrain.capacity,
                    );
                } else if other_tile.terrain.closed {
                    other_tile.terrain.temperature = SampleView {
                        air: &mut own.air,
                        archived: &mut tile.archived,
                        last_share: &mut tile.last_share,
                        boundary: &tile.boundary,
                    }
                    .temperature_share_solid(
                        other_tile.terrain.conductivity,
                        other_tile.terrain.temperature,
                        other_tile.terrain.capacity,
                    );
                } else {
                    SampleView {
                        air: &mut own.air,
                        archived: &mut tile.archived,
                        last_share: &mut tile.last_share,
                        boundary: &tile.boundary,
                    }
                    .temperature_share(
                        &mut SampleView {
                            air: &mut other.air,
                            archived: &mut other_tile.archived,
                            last_share: &mut other_tile.last_share,
                            boundary: &other_tile.boundary,
                        },
                        WINDOW_HEAT_TRANSFER_COEFFICIENT,
                    );
                }
                if other_tile.terrain.closed {
                    commands.push((
                        neighbor,
                        WorldCommand {
                            action: WorldAction::TurfExposure {
                                temperature: other_tile.terrain.temperature,
                                gas: false,
                            },
                            air: other.air.clone(),
                        },
                    ));
                } else {
                    self.activate(neighbor);
                }
                self.consider_conduction(cells, neighbor, false);
            }
            let tile: &mut TileState = self.tiles.get_mut(&id).unwrap();
            let terrain = &mut tile.terrain;
            let delta = terrain.archived - TCMB;
            if terrain.temperature > T0C
                && terrain.capacity > 0.0
                && delta.abs() > MINIMUM_TEMPERATURE_DELTA_TO_CONSIDER
            {
                let heat = (terrain.conductivity * delta)
                    * (HEAT_CAPACITY_VACUUM
                        * (terrain.capacity / (HEAT_CAPACITY_VACUUM + terrain.capacity)));
                terrain.temperature -= heat / terrain.capacity;
            }
            let temperature = if terrain.closed {
                terrain.temperature
            } else {
                cells[&id].air.temperature
            };
            if temperature < MINIMUM_TEMPERATURE_FOR_SUPERCONDUCTION {
                self.conducting.retain(|v| *v != id);
            } else if !terrain.closed {
                terrain.temperature = SampleView {
                    air: &mut cells.get_mut(&id).unwrap().air,
                    archived: &mut tile.archived,
                    last_share: &mut tile.last_share,
                    boundary: &tile.boundary,
                }
                .temperature_share_solid(
                    terrain.conductivity,
                    terrain.temperature,
                    terrain.capacity,
                );
            }
        }
        commands
    }
}
