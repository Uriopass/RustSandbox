use crate::map::{
    Intersections, LaneID, LaneKind, Lanes, LightPolicy, Road, RoadID, Roads, SpatialMap,
    TraverseDirection, Turn, TurnID, TurnPolicy,
};
use geom::{pseudo_angle, Circle};
use geom::{Vec2, Vec3};
use ordered_float::OrderedFloat;
use serde::{Deserialize, Serialize};
use slotmapd::new_key_type;
use std::collections::BTreeSet;

new_key_type! {
    pub struct IntersectionID;
}

impl IntersectionID {
    pub fn as_ffi(self) -> u64 {
        self.0.as_ffi()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Intersection {
    pub id: IntersectionID,
    pub pos: Vec3,

    turns: BTreeSet<Turn>,

    // sorted by angle
    pub roads: Vec<RoadID>,

    pub turn_policy: TurnPolicy,
    pub light_policy: LightPolicy,
}

impl Intersection {
    pub fn make(store: &mut Intersections, spatial: &mut SpatialMap, pos: Vec3) -> IntersectionID {
        let id = store.insert_with_key(|id| Intersection {
            id,
            pos,
            turns: Default::default(),
            roads: Default::default(),
            turn_policy: Default::default(),
            light_policy: Default::default(),
        });
        spatial.insert(id, pos.xy());
        id
    }

    pub fn add_road(&mut self, roads: &Roads, road: &Road) {
        self.roads.push(road.id);

        let id = self.id;
        self.roads.retain(|&id| roads.contains_key(id));
        self.roads.sort_by_key(|&road| {
            #[allow(clippy::indexing_slicing)]
            OrderedFloat(pseudo_angle(roads[road].dir_from(id)))
        });
    }

    pub fn bcircle(&self, roads: &Roads) -> Circle {
        Circle {
            center: self.pos.xy(),
            radius: self
                .roads
                .iter()
                .flat_map(|x| roads.get(*x))
                .map(|x| OrderedFloat(x.interface_from(self.id)))
                .max()
                .map(|x| x.0)
                .unwrap_or(10.0),
        }
    }

    pub fn remove_road(&mut self, road_id: RoadID) {
        self.roads.retain(|x| *x != road_id);
    }

    pub fn update_turns(&mut self, lanes: &Lanes, roads: &Roads) {
        self.turns = self
            .turn_policy
            .generate_turns(self, lanes, roads)
            .into_iter()
            .map(|(id, kind)| Turn::new(id, kind))
            .collect();

        self.turns = std::mem::take(&mut self.turns)
            .into_iter()
            .map(|mut x| {
                x.make_points(lanes, self);
                x
            })
            .collect();
    }

    pub fn update_traffic_control(&self, lanes: &mut Lanes, roads: &Roads) {
        self.light_policy.apply(self, lanes, roads);
    }

    fn check_dead_roads(&mut self, roads: &Roads) {
        let id = self.id;
        self.roads.retain(|x| {
            let v = roads.contains_key(*x);
            if !v {
                log::error!(
                    "{:?} contained unexisting {:?} when updating interface radius",
                    id,
                    x
                );
            }
            v
        });
    }

    const MIN_INTERFACE: f32 = 9.0;
    // allow slicing since we remove all roads not in self.roads
    #[allow(clippy::indexing_slicing)]
    pub fn update_interface_radius(&mut self, roads: &mut Roads) {
        let id = self.id;
        self.check_dead_roads(roads);

        for &r in &self.roads {
            let r = &mut roads[r];
            r.set_interface(id, Self::empty_interface(r.width));
        }

        if self.is_roundabout() {
            if let Some(rb) = self.turn_policy.roundabout {
                for &r in &self.roads {
                    let r = &mut roads[r];
                    r.max_interface(id, rb.radius * 1.1 + 5.0);
                }
            }
        }

        if self.roads.len() <= 1 {
            return;
        }

        for i in 0..self.roads.len() {
            let r1_id = self.roads[i];
            let r2_id = self.roads[(i + 1) % self.roads.len()];

            let r1 = &roads[r1_id];
            let r2 = &roads[r2_id];

            let min_dist =
                Self::interface_calc(r1.width, r2.width, r1.dir_from(id), r2.dir_from(id));
            roads[r1_id].max_interface(id, min_dist);
            roads[r2_id].max_interface(id, min_dist);
        }
    }

    fn interface_calc(w1: f32, w2: f32, dir1: Vec2, dir2: Vec2) -> f32 {
        let hwidth1 = w1 * 0.5;
        let hwidth2 = w2 * 0.5;

        let w = hwidth1.hypot(hwidth2);

        let d = dir1.dot(dir2).clamp(0.0, 1.0);
        let sin = (1.0 - d * d).sqrt();

        (w * 1.1 / sin).min(30.0)
    }

    pub fn empty_interface(width: f32) -> f32 {
        (width * 0.8).max(Self::MIN_INTERFACE)
    }

    pub fn interface_at(&self, roads: &Roads, width: f32, dir: Vec2) -> f32 {
        let mut max_inter = Self::empty_interface(width);
        let id = self.id;
        for &r1_id in &self.roads {
            let r1 = unwrap_cont!(roads.get(r1_id));
            max_inter = max_inter.max(Self::interface_calc(r1.width, width, r1.dir_from(id), dir));
        }
        max_inter
    }

    pub fn is_roundabout(&self) -> bool {
        self.turn_policy.roundabout.is_some() && self.roads.len() > 1
    }

    pub fn undirected_neighbors<'a>(
        &'a self,
        roads: &'a Roads,
    ) -> impl Iterator<Item = IntersectionID> + 'a {
        self.roads
            .iter()
            .flat_map(move |&x| roads.get(x).and_then(|r| r.other_end(self.id)))
    }

    pub fn vehicle_neighbours<'a>(
        &'a self,
        roads: &'a Roads,
    ) -> impl Iterator<Item = IntersectionID> + 'a {
        let id = self.id;
        self.roads.iter().flat_map(move |&x| {
            let r = roads.get(x)?;
            r.outgoing_lanes_from(id).iter().find(|(_, kind)| {
                matches!(kind, LaneKind::Driving | LaneKind::Rail | LaneKind::Bus)
            })?;
            r.other_end(id)
        })
    }

    pub fn find_turn(&self, needle: TurnID) -> Option<&Turn> {
        self.turns.get(&needle)
    }

    pub fn turns_from(
        &self,
        lane: LaneID,
    ) -> impl Iterator<Item = (TurnID, TraverseDirection)> + '_ {
        self.turns.iter().filter_map(move |Turn { id, .. }| {
            if id.src == lane {
                Some((*id, TraverseDirection::Forward))
            } else if id.bidirectional && id.dst == lane {
                Some((*id, TraverseDirection::Backward))
            } else {
                None
            }
        })
    }

    pub fn turns_to(&self, lane: LaneID) -> impl Iterator<Item = (TurnID, TraverseDirection)> + '_ {
        self.turns.iter().filter_map(move |Turn { id, .. }| {
            if id.dst == lane {
                Some((*id, TraverseDirection::Forward))
            } else if id.bidirectional && id.src == lane {
                Some((*id, TraverseDirection::Backward))
            } else {
                None
            }
        })
    }

    pub fn turns(&self) -> impl ExactSizeIterator<Item = &Turn> {
        self.turns.iter()
    }
}

debug_inspect_impl!(IntersectionID);
