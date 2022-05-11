use super::Tool;
use crate::gui::inputmap::{InputAction, InputMap};
use crate::rendering::immediate::ImmediateDraw;
use crate::uiworld::UiWorld;
use egregoria::Egregoria;
use geom::{Vec2, OBB};
use map_model::{LanePatternBuilder, ProjectFilter};

pub struct TrainstationResource {
    rotation: Vec2,
}

#[profiling::function]
pub fn trainstation(goria: &Egregoria, uiworld: &mut UiWorld) {
    let tool = *uiworld.read::<Tool>();
    if !matches!(tool, Tool::TrainStation) {
        return;
    }

    uiworld.write_or_default::<TrainstationResource>();
    let mut res = uiworld.write::<TrainstationResource>();
    let inp = uiworld.read::<InputMap>();

    let mut draw = uiworld.write::<ImmediateDraw>();
    let map = goria.map();
    let commands = &mut *uiworld.commands();

    let mpos = unwrap_ret!(inp.unprojected);

    let w = LanePatternBuilder::new().rail(true).n_lanes(1).width();

    let obb = OBB::new(mpos.xy(), res.rotation, 130.0, w + 15.0);

    let intersects = map
        .query_exact(obb, ProjectFilter::INTER | ProjectFilter::ROAD)
        .next()
        .is_some();

    let mut col = common::config().gui_primary;
    if intersects {
        col = common::config().gui_danger;
    }
    col.a = 0.5;

    draw.obb(obb, mpos.z + 0.8).color(col);

    if inp.act.contains(&InputAction::Rotate) {
        res.rotation = res.rotation.rotated_by_angle(inp.wheel * 0.1);
    }

    if inp.act.contains(&InputAction::Select) && !intersects {
        commands.map_build_trainstation(
            mpos - 65.0 * res.rotation.z(0.0),
            mpos + 65.0 * res.rotation.z(0.0),
        );
    }
}

impl Default for TrainstationResource {
    fn default() -> Self {
        Self { rotation: Vec2::X }
    }
}
