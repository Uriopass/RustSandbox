use common::AudioKind;
use geom::{BoldLine, BoldSpline, Camera, PolyLine, ShapeEnum, Spline};
use geom::{PolyLine3, Vec2, Vec3};
use simulation::map::{
    LanePatternBuilder, Map, MapProject, ProjectFilter, ProjectKind, PylonPosition, RoadSegmentKind,
};
use simulation::world_command::{WorldCommand, WorldCommands};
use simulation::Simulation;
use BuildState::{Hover, Interpolation, Start, StartInterp, Connection};
use ProjectKind::{Building, Ground, Inter, Road};

use crate::gui::{PotentialCommands, Tool};
use crate::inputmap::{InputAction, InputMap};
use crate::rendering::immediate::{ImmediateDraw, ImmediateSound};
use crate::uiworld::UiWorld;

#[derive(Copy, Clone, Debug, Default)]
pub enum BuildState {
    #[default]
    Hover,
    Start(MapProject),
    StartInterp(MapProject),
    Connection(MapProject, MapProject),
    Interpolation(Vec2, MapProject),
}

/// Road building tool
/// Allows to build roads and intersections
pub fn roadbuild(sim: &Simulation, uiworld: &mut UiWorld) {
    profiling::scope!("gui::roadbuild");
    let state = &mut *uiworld.write::<RoadBuildResource>();
    let immdraw = &mut *uiworld.write::<ImmediateDraw>();
    let immsound = &mut *uiworld.write::<ImmediateSound>();
    let potential_command = &mut *uiworld.write::<PotentialCommands>();
    let mut inp = uiworld.write::<InputMap>();
    let tool = *uiworld.read::<Tool>();
    let map = &*sim.map();
    let commands: &mut WorldCommands = &mut uiworld.commands();
    let cam = &*uiworld.read::<Camera>();

    if !tool.is_roadbuild() {
        state.build_state = Hover;
        state.height_offset = 0.0;
        return;
    }

    let nosnapping = inp.act.contains(&InputAction::NoSnapping);

    // Prepare mousepos depending on snap to grid
    let unproj = unwrap_ret!(inp.unprojected);
    let grid_size = 20.0;
    let mousepos = if state.snap_to_grid {
        let v = unproj.xy().snap(grid_size, grid_size);
        v.z(unwrap_ret!(map.environment.height(v)) + state.height_offset)
    } else if state.snap_to_angle {
        state.streight_points = state._update_points(map, unproj.up(state.height_offset));
        state.streight_points.iter()
        .filter_map(|&point| {
            let distance = point.distance(unproj);
            if distance < grid_size {Some((point, distance))} else { None }
        })
        .reduce(|acc, e| { if acc.1 < e.1 {acc} else { e } })
        .unwrap_or((unproj.up(state.height_offset), 0.0)).0
    } else {
        unproj.up(state.height_offset)
    };

    let log_camheight = cam.eye().z.log10();
    /*
    let cutoff = 3.3;

    if state.snap_to_grid && log_camheight < cutoff {
        let alpha = 1.0 - log_camheight / cutoff;
        let col = simulation::config().gui_primary.a(alpha);
        let screen = AABB::new(unproj.xy(), unproj.xy()).expand(300.0);
        let startx = (screen.ll.x / grid_size).ceil() * grid_size;
        let starty = (screen.ll.y / grid_size).ceil() * grid_size;

        let height = |p| map.terrain.height(p);
        for x in 0..(screen.w() / grid_size) as i32 {
            let x = startx + x as f32 * grid_size;
            for y in 0..(screen.h() / grid_size) as i32 {
                let y = starty + y as f32 * grid_size;
                let p = vec2(x, y);
                let p3 = p.z(unwrap_cont!(height(p)) + 0.1);
                let px = p + Vec2::x(grid_size);
                let py = p + Vec2::y(grid_size);

                immdraw
                    .line(p3, px.z(unwrap_cont!(height(px)) + 0.1), 0.3)
                    .color(col);
                immdraw
                    .line(p3, py.z(unwrap_cont!(height(py)) + 0.1), 0.3)
                    .color(col);
            }
        }
    }*/

    // If a road was placed recently (as it is async with networking) prepare the next road
    for command in uiworld.received_commands().iter() {
        if let WorldCommand::MapMakeConnection { to, .. } = command {
            if let proj @ MapProject { kind: Inter(_), .. } =
                map.project(to.pos, 0.0, ProjectFilter::ALL)
            {
                if matches!(tool, Tool::RoadbuildCurved) {
                    state.build_state = StartInterp(proj);
                } else {
                    state.build_state = Start(proj);
                }
            }
        }
    }

    if inp.just_act.contains(&InputAction::Close) && !matches!(state.build_state, Hover) {
        inp.just_act.remove(&InputAction::Close);
        state.build_state = Hover;
    }

    if inp.just_act.contains(&InputAction::UpElevation) {
        state.height_offset += 5.0;
        state.height_offset = state.height_offset.min(100.0);
    }

    if inp.just_act.contains(&InputAction::DownElevation) {
        state.height_offset -= 5.0;
        state.height_offset = state.height_offset.max(0.0);
    }

    let mut cur_proj = if !matches!(state.build_state, Connection(..)) {
        map.project(mousepos,
            (log_camheight * 5.0).clamp(1.0, 10.0),
            ProjectFilter::INTER | ProjectFilter::ROAD
        )
    } else {
        MapProject::ground(mousepos)
    }; 

    let patwidth = state.pattern_builder.width();

    if let Road(r_id) = cur_proj.kind {
        let r = &map.roads()[r_id];
        if r.points
            .first()
            .is_close(cur_proj.pos, r.interface_from(r.src) + patwidth * 0.5)
        {
            cur_proj = MapProject {
                kind: Inter(r.src),
                pos: r.points.first(),
            };
        } else if r
            .points
            .last()
            .is_close(cur_proj.pos, r.interface_from(r.dst) + patwidth * 0.5)
        {
            cur_proj = MapProject {
                kind: Inter(r.dst),
                pos: r.points.last(),
            };
        }
    }

    if nosnapping {
        cur_proj = MapProject {
            pos: mousepos,
            kind: Ground,
        }
    }

    let is_rail = state.pattern_builder.rail;

    let mut is_valid = match (state.build_state, cur_proj.kind) {
        (Hover, Building(_)) => false,
        (StartInterp(sel_proj), Ground) => {
            compatible(map, cur_proj, sel_proj)
            && check_angle(map, sel_proj, cur_proj.pos.xy(), is_rail)
        }
        (StartInterp(sel_proj), Inter(_)|Road(_)) => {
            compatible(map, sel_proj, cur_proj)
        }
        (Start(selected_proj), _) => {
            let sp = BoldLine::new(
                PolyLine::new(vec![selected_proj.pos.xy(), cur_proj.pos.xy()]),
                patwidth * 0.5,
            );

            compatible(map, cur_proj, selected_proj)
            && check_angle(map, selected_proj, cur_proj.pos.xy(), is_rail)
            && check_angle(map, cur_proj, selected_proj.pos.xy(), is_rail)
            && !check_intersect(
                map, &ShapeEnum::BoldLine(sp),
                (selected_proj.pos.z + cur_proj.pos.z) / 2.0,
                cur_proj.kind, selected_proj.kind,
            )
        }
        (Connection(src, dst), _) => {
            let sp = Spline {
                from: src.pos.xy(), to: dst.pos.xy(),
                from_derivative: (cur_proj.pos.xy() - src.pos.xy()) * std::f32::consts::FRAC_1_SQRT_2,
                to_derivative: (dst.pos.xy() - cur_proj.pos.xy()) * std::f32::consts::FRAC_1_SQRT_2,
            };

            compatible(map, dst, src)
            && check_angle(map, src, cur_proj.pos.xy(), is_rail)
            && check_angle(map, dst, cur_proj.pos.xy(), is_rail)
            && !sp.is_steep(state.pattern_builder.width())
            && !check_intersect(
                map, &ShapeEnum::BoldSpline(BoldSpline::new(sp, patwidth * 0.5)),
                (src.pos.z + dst.pos.z) / 2.0,
                src.kind, dst.kind,
            )
        }
        (Interpolation(interpoint, selected_proj), _) => {
            let sp = Spline {
                from: selected_proj.pos.xy(),
                to: cur_proj.pos.xy(),
                from_derivative: (interpoint - selected_proj.pos.xy()) * std::f32::consts::FRAC_1_SQRT_2,
                to_derivative: (cur_proj.pos.xy() - interpoint) * std::f32::consts::FRAC_1_SQRT_2,
            };

            compatible(map, cur_proj, selected_proj)
            && check_angle(map, selected_proj, interpoint, is_rail)
            && check_angle(map, cur_proj, interpoint, is_rail)
            && !sp.is_steep(state.pattern_builder.width())
            && !check_intersect(
                map, &ShapeEnum::BoldSpline(BoldSpline::new(sp, patwidth * 0.5)),
                (selected_proj.pos.z + cur_proj.pos.z) / 2.0,
                selected_proj.kind, cur_proj.kind,
            )
        }
        _ => true,
    };

    let build_args = match state.build_state {
        StartInterp(selected_proj) if !cur_proj.is_ground() => {
            Some((selected_proj, cur_proj, None, state.pattern_builder.build()))
        }
        Start(selected_proj) => {
            Some((selected_proj, cur_proj, None, state.pattern_builder.build()))
        },
        Connection(src, dst) => {
            Some((src, dst, Some(cur_proj.pos.xy()), state.pattern_builder.build()))
        }

        Interpolation(interpoint, selected_proj) => {
            let inter = Some(interpoint);
            Some((selected_proj, cur_proj, inter, state.pattern_builder.build()))
        }
        _ => None,
    };
    potential_command.0.clear();

    let mut points = None;

    if let Some((src, dst, inter, pat)) = build_args {
        potential_command.set(WorldCommand::MapMakeConnection {
            from: src, to: dst, inter, pat,
        });

        let connection_segment = match inter {
            Some(x) => RoadSegmentKind::from_elbow(src.pos.xy(), dst.pos.xy(), x),
            None => RoadSegmentKind::Straight,
        };

        let (p, err) = simulation::map::Road::generate_points(
            src.pos,
            dst.pos,
            connection_segment,
            is_rail,
            &map.environment,
        );
        points = Some(p);
        if err.is_some() {
            is_valid = false;
        }
    }

    state.update_drawing(map, immdraw, cur_proj, patwidth, is_valid, points);

    if is_valid && inp.just_act.contains(&InputAction::Select) {
        log::info!("left clicked with state {:?} and {:?}", state.build_state, cur_proj.kind);

        match (state.build_state, cur_proj.kind) {
            (Hover, Ground|Road(_)|Inter(_)) => {
                // Hover selection
                if tool == Tool::RoadbuildCurved {
                    state.build_state = StartInterp(cur_proj);
                } else {
                    state.build_state = Start(cur_proj);
                }
            }
            (StartInterp(v), Ground) => {
                // Set interpolation point
                state.build_state = Interpolation(mousepos.xy(), v);
            }
            (StartInterp(p), Road(_)|Inter(_)) => {
                // Set interpolation point
                state.build_state = Connection(p, cur_proj);
            }
            
            (Start(_), _) => {
                // Straight connection to something
                immsound.play("road_lay", AudioKind::Ui);
                if let Some(wc) = potential_command.0.drain(..).next() {
                    commands.push(wc);
                }
                state.build_state = Hover;
            }
            (Connection(_, _), _) => {
                immsound.play("road_lay", AudioKind::Ui);
                if let Some(wc) = potential_command.0.drain(..).next() {
                    commands.push(wc);
                }
                state.build_state = Hover;
            }
            (Interpolation(_, _), _) => {
                // Interpolated connection to something
                immsound.play("road_lay", AudioKind::Ui);
                if let Some(wc) = potential_command.0.drain(..).next() {
                    commands.push(wc);
                }
                state.build_state = Hover;
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub struct RoadBuildResource {
    pub build_state: BuildState,
    pub pattern_builder: LanePatternBuilder,
    pub snap_to_grid: bool,
    pub snap_to_angle: bool,
    pub height_offset: f32,
    pub streight_points: Vec<Vec3>,
}

fn check_angle(map: &Map, from: MapProject, to: Vec2, is_rail: bool) -> bool {
    let max_turn_angle = if is_rail {
        0.0
    } else {
        30.0 * std::f32::consts::PI / 180.0
    };

    match from.kind {
        Inter(i) => {
            let Some(inter) = map.intersections().get(i) else {return false;};
            let dir = (to - inter.pos.xy()).normalize();

            inter.roads.iter()
            .map(|road_id| map.roads()[*road_id].dir_from(i))
            .any(|v| {v.angle(dir).abs() >= max_turn_angle})
        }
        Road(r) => {
            let Some(r) = map.roads().get(r) else { return false; };
            let (proj, _, rdir1) = r.points().project_segment_dir(from.pos);
            let rdir2 = -rdir1;
            let dir = (to - proj.xy()).normalize();

            rdir1.xy().angle(dir).abs() >= max_turn_angle
            && rdir2.xy().angle(dir).abs() >= max_turn_angle
        }
        _ => true,
    }
}

fn compatible(map: &Map, x: MapProject, y: MapProject) -> bool {
    if x.pos.distance(y.pos) < 10.0 {
        return false;
    }
    match (x.kind, y.kind) {
        (Ground, Ground)
        | (Ground, Road(_))
        | (Ground, Inter(_))
        | (Road(_), Ground)
        | (Inter(_), Ground) => true,
        (Road(id), Road(id2)) => id != id2,
        (Inter(id), Inter(id2)) => id != id2,
        (Inter(id_inter), Road(id_road)) | (Road(id_road), Inter(id_inter)) => {
            let r = &map.roads()[id_road];
            r.src != id_inter && r.dst != id_inter
        }
        _ => false,
    }
}

/// Check if the given shape intersects with any existing road or intersection
fn check_intersect(
    map: &Map,
    obj: &ShapeEnum,
    z: f32,
    start: ProjectKind,
    end: ProjectKind,
) -> bool {
    map.spatial_map()
        .query(obj, ProjectFilter::ROAD | ProjectFilter::INTER)
        .any(move |x| {
            if let Road(rid) = x {
                let r = &map.roads()[rid];
                if (r.points.first().z - z).abs() > 1.0 || (r.points.last().z - z).abs() > 1.0 {
                    return false;
                }
                if let Inter(id) = start {
                    if r.src == id || r.dst == id {
                        return false;
                    }
                }
                if let Inter(id) = end {
                    if r.src == id || r.dst == id {
                        return false;
                    }
                }
            }
            x != start && x != end
        })
}

impl RoadBuildResource {
    pub fn update_drawing(
        &self,
        map: &Map,
        immdraw: &mut ImmediateDraw,
        proj: MapProject,
        patwidth: f32,
        is_valid: bool,
        points: Option<PolyLine3>,
    ) {
        let mut proj_pos = proj.pos;
        proj_pos.z += 0.1;
        let col = if is_valid {
            simulation::config().gui_primary
        } else {
            simulation::config().gui_danger
        };

        self.streight_points.iter().for_each(|p|{
            immdraw.circle(*p, 2.0);
        });

        let p = match self.build_state {
            Hover => {
                immdraw.circle(proj_pos, patwidth * 0.5).color(col);
                return;
            }
            StartInterp(x) if proj.kind.is_ground() => {
                let dir = unwrap_or!((proj_pos - x.pos).try_normalize(), {
                    immdraw.circle(proj_pos, patwidth * 0.5).color(col);
                    return;
                });
                let mut poly = Vec::with_capacity(33);
                for i in 0..=32 {
                    let ang = std::f32::consts::PI * i as f32 * (2.0 / 32.0);
                    let mut v = Vec3::from_angle(ang, dir.z);
                    let center = if v.dot(dir) < 0.0 { x.pos } else { proj.pos };

                    v = v * patwidth * 0.5;
                    v.z = 0.0;
                    v += center;

                    poly.push(v);
                }
                immdraw.polyline(poly, 3.0, true).color(col);

                return;
            }
            _ => unwrap_ret!(points),
        };

        for PylonPosition {
            terrain_height,
            pos,
            ..
        } in simulation::map::Road::pylons_positions(&p, &map.environment)
        {
            immdraw
                .circle(pos.xy().z(terrain_height + 0.1), patwidth * 0.5)
                .color(col);
        }

        immdraw.circle(p.first(), patwidth * 0.5).color(col);
        immdraw.circle(p.last(), patwidth * 0.5).color(col);
        immdraw.polyline(p.into_vec(), patwidth, false).color(col);
    }

    pub fn _update_points(
        &self,
        map: &Map,
        mousepos: Vec3,
    ) -> Vec<Vec3> {
        let (start, end) = match self.build_state {
            Hover | Interpolation(_, _) => { return vec![]; },
            Connection(src, dst) => (src, dst),
            Start(sel_proj)|StartInterp(sel_proj) => (sel_proj, MapProject::ground(mousepos)),
        };

        match (start.kind, end.kind) {
            (Inter(id0), Inter(id1)) => {
                let Some(inter0) = map.intersections().get(id0) else {return vec![]};
                let Some(inter1) = map.intersections().get(id1) else {return vec![]};
                
                inter0.roads.iter().flat_map(|i| inter1.roads.iter().map(move |j| (i, j)))
                .map( |road_ids| (&map.roads()[*road_ids.0], &map.roads()[*road_ids.1]))
                .filter_map(|roads| {
                    let p = Vec2::line_line_intersection(
                        inter0.pos.xy(), roads.0.get_straight_connection_point(id0),
                        inter1.pos.xy(), roads.1.get_straight_connection_point(id1));

                    if let Some(h) = map.environment.height(p)
                    { Some(p.z(h)) } else { None }
                }).collect()
            },

            (Inter(id), Ground) |
            (Ground, Inter(id)) => {
                let Some(inter) = map.intersections().get(id) else {return vec![]};

                inter.roads.iter()
                .map(|&road_id| &map.roads()[road_id])
                .filter_map(|road| {
                    let p = Vec2::line_closed_point(mousepos.xy(),
                        road.get_straight_connection_point(id), inter.pos.xy());
                    
                    if let Some(h) = map.environment.height(p)
                    { Some(p.z(h)) } else { None }
                }).collect()
            }

            (Inter(inter_id), Road(road_id)) |
            (Road(road_id), Inter(inter_id))
            if self.pattern_builder.rail => {
                let Some(inter) = map.intersections().get(inter_id) else {return vec![]};
                let Some(road) = map.roads().get(road_id) else {return vec![]};

                let pos = if start.kind == Road(road_id) {start.pos} else {end.pos};
                let (pos, _, dir) = road.points().project_segment_dir(pos);

                inter.roads.iter()
                .map(|&road_id| &map.roads()[road_id])
                .filter_map(|road| {
                    let p = Vec2::line_line_intersection(
                        road.get_straight_connection_point(inter_id), inter.pos.xy(),
                        pos.xy(), pos.xy()+dir.xy(),
                    );
                    if let Some(h) = map.environment.height(p)
                    { Some(p.z(h)) } else { None }
                }).collect()
            }

            (Road(id), Ground) |
            (Ground, Road(id))
            if self.pattern_builder.rail => {
                let Some(road) = map.roads().get(id) else {return vec![]};
                
                let pos = if start.kind == Road(id) {start.pos} else {end.pos};
                let (pos, _, dir) = road.points().project_segment_dir(pos);

                let p = Vec2::line_closed_point(mousepos.xy(), pos.xy()+dir.xy(), pos.xy());                
                if let Some(h) = map.environment.height(p) 
                { vec![p.z(h)] } else { vec![] }
            }
            (Road(id0), Road(id1)) if self.pattern_builder.rail => {
                let Some(road0) = map.roads().get(id0) else {return vec![]};
                let Some(road1) = map.roads().get(id1) else {return vec![]};

                let (pos0, _, dir0) = road0.points().project_segment_dir(start.pos);
                let (pos1, _, dir1) = road1.points().project_segment_dir(end.pos);

                let p = Vec2::line_line_intersection(
                    pos0.xy(), pos0.xy()+dir0.xy(), 
                    pos1.xy(), pos1.xy()+dir1.xy());

                if let Some(h) = map.environment.height(p)
                { vec![p.z(h)] } else { vec![] }
            },
            _ => { vec![] }
        }
    }
}
