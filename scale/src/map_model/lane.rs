use crate::geometry::polyline::PolyLine;
use crate::geometry::Vec2;
use crate::map_model::{IntersectionID, Road, TrafficControl, TraverseDirection};
use either::Either;
use imgui_inspect_derive::*;
use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

new_key_type! {
    pub struct LaneID;
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LaneKind {
    Driving,
    Biking,
    Parking,
    Bus,
    Construction,
    Walking,
}

impl LaneKind {
    pub fn vehicles(self) -> bool {
        matches!(self, LaneKind::Driving | LaneKind::Biking | LaneKind::Bus)
    }

    pub fn needs_light(self) -> bool {
        matches!(self, LaneKind::Driving | LaneKind::Biking | LaneKind::Bus)
    }

    pub fn width(self) -> f32 {
        match self {
            LaneKind::Driving | LaneKind::Biking | LaneKind::Bus => 8.0,
            LaneKind::Parking => 4.0,
            LaneKind::Construction => 4.0,
            LaneKind::Walking => 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaneDirection {
    Forward,
    Backward,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Lane {
    pub id: LaneID,
    pub kind: LaneKind,

    pub control: TrafficControl,

    /// Src and dst implies direction
    pub src: IntersectionID,
    pub dst: IntersectionID,

    /// Src and dst implies direction
    pub src_dir: Vec2,
    pub dst_dir: Vec2,

    /// Always from src to dst
    pub points: PolyLine,
    pub width: f32,

    /// Length from intersection to intersection
    pub inter_length: f32,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanePattern {
    pub lanes_forward: Vec<LaneKind>,
    pub lanes_backward: Vec<LaneKind>,
}

#[derive(Clone, Copy, Inspect)]
pub struct LanePatternBuilder {
    pub n_lanes: u32,
    pub sidewalks: bool,
    pub parking: bool,
    pub one_way: bool,
}

impl Default for LanePatternBuilder {
    fn default() -> Self {
        LanePatternBuilder {
            n_lanes: 1,
            sidewalks: true,
            parking: true,
            one_way: false,
        }
    }
}

impl LanePatternBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn n_lanes(mut self, n_lanes: u32) -> Self {
        assert!(n_lanes > 0);
        self.n_lanes = n_lanes;
        self
    }

    pub fn sidewalks(mut self, sidewalks: bool) -> Self {
        self.sidewalks = sidewalks;
        self
    }

    pub fn parking(mut self, parking: bool) -> Self {
        self.parking = parking;
        self
    }

    pub fn one_way(mut self, one_way: bool) -> Self {
        self.one_way = one_way;
        self
    }

    pub fn width(self) -> f32 {
        let mut w = 0.0;
        if self.sidewalks {
            w += LaneKind::Walking.width() * 2.0;
        }
        if self.parking {
            w += LaneKind::Parking.width() * 2.0;
        }
        w += self.n_lanes as f32 * 2.0 * LaneKind::Driving.width();
        w
    }

    pub fn build(self) -> LanePattern {
        let mut backward = if self.one_way {
            vec![]
        } else {
            (0..self.n_lanes).map(|_| LaneKind::Driving).collect()
        };

        let mut forward: Vec<_> = (0..self.n_lanes).map(|_| LaneKind::Driving).collect();

        if self.parking {
            backward.push(LaneKind::Parking);
            forward.push(LaneKind::Parking);
        }

        if self.sidewalks {
            backward.push(LaneKind::Walking);
            forward.push(LaneKind::Walking);
        }

        LanePattern {
            lanes_backward: backward,
            lanes_forward: forward,
        }
    }
}

impl Lane {
    pub fn get_inter_node_pos(&self, id: IntersectionID) -> Vec2 {
        match (id, self.points.as_slice()) {
            (x, [p, ..]) if x == self.src => *p,
            (x, [.., p]) if x == self.dst => *p,
            _ => panic!("Oh no"),
        }
    }

    pub fn gen_pos(&mut self, parent_road: &Road, dist_from_bottom: f32) {
        let lane_dist = self.width * 0.5 + dist_from_bottom - parent_road.width * 0.5;

        self.points.clear();
        for v in parent_road.interpolation_splines() {
            let spline = match v {
                Either::Left(s) => s,
                Either::Right(segment) => {
                    let nor = (segment.dst - segment.src).perpendicular();
                    if self.points.is_empty() {
                        self.points.push(segment.src + nor * lane_dist);
                    }
                    self.points.push(segment.dst + nor * lane_dist);
                    continue;
                }
            };

            if self.points.is_empty() {
                let nor = -spline.from_derivative.normalize().perpendicular();
                self.points.push(spline.from + nor * lane_dist);
            }

            let points: Vec<Vec2> = spline.smart_points(1.0).collect();
            for window in points.windows(3) {
                let a = window[0];
                let elbow = window[1];
                let c = window[2];

                let x = unwrap_or!((elbow - a).try_normalize(), continue);
                let y = unwrap_or!((elbow - c).try_normalize(), continue);

                let mut dir = (x + y).try_normalize().unwrap_or(-x.perpendicular());

                if x.perp_dot(y) < 0.0 {
                    dir = -dir;
                }

                let mul = 1.0 + (1.0 + x.dot(y).min(0.0)) * (std::f32::consts::SQRT_2 - 1.0);

                let nor = mul * lane_dist * dir;
                self.points.push(elbow + nor);
            }

            let nor = -spline.to_derivative.normalize().perpendicular();
            self.points.push(spline.to + nor * lane_dist);
        }

        if self.dir_from(parent_road.src) == TraverseDirection::Backward {
            self.points.reverse();
        }

        self.inter_length = parent_road.length;
    }

    pub fn dist2_to(&self, p: Vec2) -> f32 {
        (self.points.project(p).unwrap() - p).magnitude2()
    }

    pub fn dir_from(&self, i: IntersectionID) -> TraverseDirection {
        if self.src == i {
            TraverseDirection::Forward
        } else {
            TraverseDirection::Backward
        }
    }

    pub fn orientation_from(&self, id: IntersectionID) -> Vec2 {
        if id == self.src {
            self.src_dir
        } else {
            self.dst_dir
        }
    }
}
