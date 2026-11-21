//! Shared widgets: theme, stage stepper, magnifier and overlay painters.

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, Vec2};
use plot2csv_core::project::{Stage, StageStatus};

pub const ACCENT: Color32 = Color32::from_rgb(255, 195, 0);
pub const OK: Color32 = Color32::from_rgb(46, 160, 67);
pub const WARN: Color32 = Color32::from_rgb(214, 120, 0);
pub const INFO: Color32 = Color32::from_rgb(40, 120, 220);

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.selection.bg_fill = if dark { Color32::from_rgb(120, 90, 0) } else { Color32::from_rgb(255, 214, 102) };
    v.selection.stroke = Stroke::new(1.0, if dark { Color32::WHITE } else { Color32::BLACK });
    v.hyperlink_color = INFO;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(7.0, 3.0);
        // egui's defaults + 2 pt. Rebuilt from the defaults (not the current sizes) since
        // this runs again on every theme toggle.
        s.text_styles = egui::style::default_text_styles()
            .into_iter()
            .map(|(style, mut font)| {
                font.size += 2.0;
                (style, font)
            })
            .collect();
    });
}

/// Colour + short text for a stage status badge.
pub fn status_badge(s: StageStatus) -> (Color32, &'static str) {
    match s {
        StageStatus::Pending => (Color32::from_gray(150), "pending"),
        StageStatus::Proposed => (INFO, "review"),
        StageStatus::Approved => (OK, "approved"),
        StageStatus::Stale => (WARN, "stale"),
    }
}

/// Circled stage numbers, indexed like `Stage::ALL`.
pub const STAGE_NUMS: [&str; 7] = ["①", "②", "③", "④", "⑤", "⑥", "⑦"];

/// The ① Import › ② Correct › … bar. Returns a stage the user clicked.
pub fn stepper(
    ui: &mut egui::Ui,
    current: Stage,
    status: impl Fn(Stage) -> StageStatus,
    can_open: impl Fn(Stage) -> bool,
) -> Option<Stage> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, st) in Stage::ALL.iter().enumerate() {
            if i > 0 {
                ui.label(egui::RichText::new("›").color(Color32::from_gray(140)).size(18.0));
            }
            let (col, word) = status_badge(status(*st));
            let enabled = can_open(*st);
            let selected = *st == current;
            let text = format!("{} {}", STAGE_NUMS[i], st.title());
            let mut rt = egui::RichText::new(text).size(16.0);
            if selected {
                rt = rt.strong();
            }
            let btn = egui::Button::new(rt).selected(selected).min_size(Vec2::new(0.0, 24.0));
            let r = ui.add_enabled(enabled, btn);
            // status dot
            let dot = Pos2::new(r.rect.right() - 5.0, r.rect.top() + 5.0);
            ui.painter().circle_filled(dot, 3.5, col);
            let r = r.on_hover_text(format!("{}: {}", st.title(), word));
            if r.clicked() {
                clicked = Some(*st);
            }
        }
    });
    clicked
}

/// Zoomed view of `tex` around image point `center` with a crosshair.
pub fn magnifier(ui: &mut egui::Ui, tex: Option<&TextureHandle>, image_size: Vec2, center: Option<Pos2>, zoom: f32, side: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_gray(245));
    if let (Some(tex), Some(c)) = (tex, center) {
        if image_size.x > 0.0 {
            let half = side / zoom / 2.0;
            let uv = Rect::from_min_max(
                Pos2::new((c.x - half) / image_size.x, (c.y - half) / image_size.y),
                Pos2::new((c.x + half) / image_size.x, (c.y + half) / image_size.y),
            );
            p.image(tex.id(), rect, uv, Color32::WHITE);
        }
    }
    let cs = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
    p.line_segment([Pos2::new(rect.center().x, rect.top()), Pos2::new(rect.center().x, rect.bottom())], cs);
    p.line_segment([Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)], cs);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(120)), StrokeKind::Inside);
    rect
}

/// A labelled marker (calibration point, data point, handle).
pub fn marker(p: &egui::Painter, at: Pos2, color: Color32, radius: f32, label: Option<&str>, selected: bool) {
    if selected {
        p.circle_filled(at, radius + 3.0, Color32::from_rgba_unmultiplied(0, 255, 0, 120));
    }
    p.circle_filled(at, radius, color);
    p.circle_stroke(at, radius, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 160)));
    if let Some(l) = label {
        let pos = at + Vec2::new(radius + 3.0, -radius - 3.0);
        let font = FontId::proportional(16.0);
        p.text(pos + Vec2::new(1.0, 1.0), Align2::LEFT_BOTTOM, l, font.clone(), Color32::from_rgba_unmultiplied(255, 255, 255, 200));
        p.text(pos, Align2::LEFT_BOTTOM, l, font, color);
    }
}

/// A crosshair-style calibration marker like WPD's.
pub fn cal_marker(p: &egui::Painter, at: Pos2, color: Color32, label: &str, selected: bool) {
    let r = 7.0;
    if selected {
        p.circle_filled(at, r + 4.0, Color32::from_rgba_unmultiplied(0, 255, 0, 110));
    }
    let s = Stroke::new(2.0, color);
    p.line_segment([at - Vec2::new(r, 0.0), at + Vec2::new(r, 0.0)], s);
    p.line_segment([at - Vec2::new(0.0, r), at + Vec2::new(0.0, r)], s);
    p.circle_stroke(at, 3.0, s);
    let font = FontId::proportional(17.0);
    let pos = at + Vec2::new(6.0, 6.0);
    p.text(pos + Vec2::new(1.0, 1.0), Align2::LEFT_TOP, label, font.clone(), Color32::WHITE);
    p.text(pos, Align2::LEFT_TOP, label, font, color);
}

/// Text with a translucent backing box, for canvas labels.
pub fn label_box(p: &egui::Painter, at: Pos2, anchor: Align2, text: &str, color: Color32) {
    let galley = p.layout_no_wrap(text.to_string(), FontId::proportional(15.0), color);
    let rect = anchor.anchor_size(at, galley.size()).expand(2.0);
    p.rect_filled(rect, CornerRadius::same(2), Color32::from_rgba_unmultiplied(255, 255, 255, 215));
    p.galley(rect.min + Vec2::splat(2.0), galley, color);
}

/// A small section header in the sidebar.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).strong().size(16.0));
}

/// A colour swatch button; returns its response.
pub fn swatch(ui: &mut egui::Ui, c: [u8; 3], size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(3), c32(c));
    ui.painter().rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Inside);
    resp
}

/// Primary action button with accent fill.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(egui::RichText::new(text).strong().color(Color32::BLACK))
        .fill(ACCENT)
        .min_size(Vec2::new(0.0, 28.0));
    ui.add_enabled(enabled, btn)
}
