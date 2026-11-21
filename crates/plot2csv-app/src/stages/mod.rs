//! One module per pipeline stage. Each adds `*_sidebar`, `*_canvas` and
//! `auto_*` methods to `Plot2CsvApp`.

mod axes;
mod correct;
mod data;
mod export;
mod import;
mod region;
mod text;

pub use axes::calibration_values_ui as axes_values_ui;
pub use axes::AxesUi;
pub use correct::CorrectUi;
pub use data::DataUi;
pub use export::ExportUi;
pub use region::RegionUi;
pub use text::{OcrPurpose, OcrTask, TextUi};

use egui::Pos2;
use plot2csv_core::geom::Pt;

pub fn pt(p: Pos2) -> Pt {
    Pt::new(p.x as f64, p.y as f64)
}

pub fn pos(p: Pt) -> Pos2 {
    Pos2::new(p.x as f32, p.y as f32)
}

/// Was an arrow key pressed this frame (a nudge, outside text fields)?
pub fn arrow_pressed(ctx: &egui::Context) -> bool {
    arrow_nudge(ctx).is_some()
}

/// Arrow-key nudge in image pixels (Shift = 10×). Returns None when no arrow is pressed.
pub fn arrow_nudge(ctx: &egui::Context) -> Option<egui::Vec2> {
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    ctx.input(|i| {
        let step = if i.modifiers.shift { 10.0 } else if i.modifiers.alt { 0.25 } else { 1.0 };
        let mut d = egui::Vec2::ZERO;
        if i.key_pressed(egui::Key::ArrowLeft) {
            d.x -= step;
        }
        if i.key_pressed(egui::Key::ArrowRight) {
            d.x += step;
        }
        if i.key_pressed(egui::Key::ArrowUp) {
            d.y -= step;
        }
        if i.key_pressed(egui::Key::ArrowDown) {
            d.y += step;
        }
        (d != egui::Vec2::ZERO).then_some(d)
    })
}
