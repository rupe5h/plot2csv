//! ⑤ Axes: calibration points + values for every plot type.

use egui::{Color32, Pos2, Stroke};
use plot2csv_core::axes::{Axes, AxesKind, AxesParams, Calibration};
use plot2csv_core::geom::Pt;
use plot2csv_core::project::{Stage, TextRole};

use super::region::to_rect;
use super::{arrow_nudge, pos, pt};
use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::dialogs::Dialog;
use crate::icons::{self, Icon};
use crate::widgets;

#[derive(Default)]
pub struct AxesUi {
    /// Index of the calibration point the next click will place.
    pub placing: Option<usize>,
    pub selected: Option<usize>,
    drag: Option<usize>,
    pub focus: Option<Pos2>,
    pub show_grid: bool,
}

impl AxesUi {
    /// Drop the selection and drag (after undo/redo).
    pub fn clear_transient(&mut self) {
        self.selected = None;
        self.drag = None;
    }
}

pub fn point_color(kind: AxesKind, i: usize) -> Color32 {
    match kind {
        AxesKind::Xy => {
            if i < 2 {
                Color32::from_rgb(220, 30, 30)
            } else {
                Color32::from_rgb(20, 150, 40)
            }
        }
        AxesKind::Polar if i == 0 => Color32::from_rgb(20, 150, 40),
        _ => Color32::from_rgb(220, 30, 30),
    }
}

/// "Nice" tick values covering [lo, hi].
pub fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let span = hi - lo;
    if !(span > 0.0) || !span.is_finite() {
        return vec![];
    }
    let raw = span / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let start = (lo / step).ceil() * step;
    let mut out = Vec::new();
    let mut v = start;
    while v <= hi + 1e-9 * span && out.len() < 50 {
        out.push(if v.abs() < 1e-12 * step { 0.0 } else { v });
        v += step;
    }
    out
}

impl Plot2CsvApp {
    pub fn auto_axes(&mut self, _ctx: &egui::Context) {
        let Some(id) = self.project.active_panel else { return };
        let Some(panel) = self.project.panel(id).cloned() else { return };
        let Some(g) = self.imgs.corrected_gray.clone() else { return };
        let items = self.project.work(id).and_then(|w| w.text.value.clone()).unwrap_or_default();
        let r = plot2csv_vision::axes_infer::infer_axes(&g, &panel, &items);
        let w = self.project.work_mut(id);
        w.evidence = r.evidence;
        w.axes.propose(r.axes, r.note);
        self.axes_ui = AxesUi { show_grid: true, ..Default::default() };
        self.resume_placing();
    }

    /// Ask for the next calibration point when the active calibration is incomplete.
    pub fn resume_placing(&mut self) {
        let axes = self.project.active_work().and_then(|w| w.axes.value.as_ref());
        self.axes_ui.placing = axes.filter(|a| !a.points_complete()).map(|a| a.points.len());
    }

    pub fn axes_mut(&mut self) -> Option<&mut Axes> {
        let id = self.project.active_panel?;
        let kind = self.project.panel(id).map(|p| p.kind).unwrap_or_default();
        let h = self.imgs.corrected_size().y as f64;
        let w = self.project.work_mut(id);
        if w.axes.value.is_none() {
            w.axes.value = Some(Axes::new(kind, h));
        }
        w.axes.value.as_mut()
    }

    /// Change the plot type of the active panel (from the Choose Plot Type dialog).
    pub fn set_plot_kind(&mut self, ctx: &egui::Context, kind: AxesKind) {
        let Some(id) = self.project.active_panel else { return };
        if let Some(ps) = self.project.regions.value.as_mut() {
            if let Some(p) = ps.iter_mut().find(|p| p.id == id) {
                p.kind = kind;
            }
        }
        if self.stage >= Stage::Axes {
            self.stage = Stage::Axes;
            self.run_auto(ctx, Stage::Axes);
        }
    }

    pub fn axes_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        widgets::section(ui, "⑤ Axes calibration");
        let Some(id) = self.project.active_panel else { return };
        let note = self.project.work(id).map(|w| w.axes.note.clone()).unwrap_or_default();
        if !note.is_empty() {
            ui.label(egui::RichText::new(note).color(widgets::INFO));
        }
        let kind = self.project.panel(id).map(|p| p.kind).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("Plot type:");
            ui.label(egui::RichText::new(kind.label()).strong());
            if ui.small_button("Change…").clicked() {
                self.dialog = Some(Dialog::PlotType);
            }
        });
        ui.label(egui::RichText::new(kind.point_help()).weak());
        let Some(axes) = self.axes_mut().cloned() else { return };
        let names = kind.point_names();
        let mut changed = false;
        let mut new_axes = axes.clone();

        // Point list
        if !names.is_empty() {
            egui::Grid::new("calpts").num_columns(3).spacing([8.0, 2.0]).show(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    let col = point_color(kind, i);
                    let sel = self.axes_ui.selected == Some(i);
                    if ui.selectable_label(sel, egui::RichText::new(*n).color(col).strong()).clicked() {
                        self.axes_ui.selected = Some(i);
                        self.axes_ui.focus = axes.points.get(i).map(|p| pos(*p));
                    }
                    match axes.points.get(i) {
                        Some(p) => ui.label(egui::RichText::new(format!("({:.1}, {:.1})", p.x, p.y)).monospace()),
                        None => ui.label(egui::RichText::new("not placed").weak()),
                    };
                    if self.axes_ui.placing == Some(i) {
                        ui.label(egui::RichText::new("◀ click on image").color(widgets::WARN));
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
            ui.horizontal_wrapped(|ui| {
                let lbl = if axes.points.is_empty() { "Place points" } else { "Re-place all points" };
                if ui.button(lbl).clicked() {
                    new_axes.points.clear();
                    self.axes_ui.placing = Some(0);
                    changed = true;
                }
                if self.axes_ui.placing.is_some() && ui.button("Stop placing").clicked() {
                    self.axes_ui.placing = None;
                }
                if ui.button("Values…").on_hover_text("Open the calibration values dialog").clicked() {
                    self.dialog = Some(Dialog::CalibrationValues);
                }
            });
        }
        ui.add_space(4.0);
        changed |= calibration_values_ui(ui, &mut new_axes);
        ui.add_space(4.0);
        match new_axes.calibrate() {
            Ok(cal) => {
                ui.horizontal_top(|ui| {
                    ui.add(icons::image_tinted(ui, Icon::CircleCheck, widgets::OK));
                    ui.add(
                        egui::Label::new(egui::RichText::new("Calibration OK — hover the image to check values.").color(widgets::OK))
                            .wrap(),
                    );
                });
                ui.checkbox(&mut self.axes_ui.show_grid, "Overlay calibrated grid");
                self.residuals_ui(ui, &cal);
                if let Calibration::Xy(c) = &cal {
                    egui::CollapsingHeader::new("Transformation equations").show(ui, |ui| {
                        ui.label(egui::RichText::new(c.equations()).monospace().small());
                    });
                }
            }
            Err(e) => {
                ui.horizontal_top(|ui| {
                    ui.add(icons::image_tinted(ui, Icon::CircleXmark, widgets::WARN));
                    ui.add(egui::Label::new(egui::RichText::new(e.to_string()).color(widgets::WARN)).wrap());
                });
            }
        }
        if changed {
            if let Some(a) = self.axes_mut() {
                *a = new_axes;
            }
            self.edited(Stage::Axes);
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Tip: drag a point to move it; arrow keys nudge the selected point (Shift ×10, Alt ×¼).").weak().small(),
        );
        let _ = ctx;
    }

    fn residuals_ui(&self, ui: &mut egui::Ui, cal: &Calibration) {
        let Some(w) = self.project.active_work() else { return };
        let items = w.text.value.clone().unwrap_or_default();
        let ev = &w.evidence;
        if ev.x_matches.is_empty() && ev.y_matches.is_empty() {
            return;
        }
        egui::CollapsingHeader::new("Tick label check").default_open(false).show(ui, |ui| {
            egui::Grid::new("resid").num_columns(4).striped(true).show(ui, |ui| {
                ui.label("Axis");
                ui.label("Label");
                ui.label("Read as");
                ui.label("Δ");
                ui.end_row();
                let y_mid = ev.x_axis.map(|a| a[0].y).unwrap_or(0.0);
                let x_mid = ev.y_axis.map(|a| a[0].x).unwrap_or(0.0);
                for (axis, matches) in [("X", &ev.x_matches), ("Y", &ev.y_matches)] {
                    for &(px, v, ti) in matches.iter() {
                        let p = if axis == "X" { Pt::new(px, y_mid) } else { Pt::new(x_mid, px) };
                        let d = cal.to_data(p);
                        let got = if axis == "X" { d[0] } else { d.get(1).copied().unwrap_or(d[0]) };
                        ui.label(axis);
                        ui.label(items.get(ti).map(|t| t.text.clone()).unwrap_or_default());
                        ui.label(format!("{:.4}", got));
                        let rel = if v != 0.0 { ((got - v) / v).abs() } else { (got - v).abs() };
                        ui.label(egui::RichText::new(format!("{:.2}%", rel * 100.0)).color(if rel < 0.01 {
                            widgets::OK
                        } else {
                            widgets::WARN
                        }));
                        ui.end_row();
                    }
                }
            });
        });
    }

    pub fn axes_canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, view: &CanvasView, interactive: bool) {
        let painter = ui.painter_at(view.rect);
        let Some(id) = self.project.active_panel else { return };
        let Some(panel) = self.project.panel(id).cloned() else { return };
        let work = self.project.work(id).cloned();
        let kind = panel.kind;
        let inner_s = view.rect_to_screen(to_rect(&panel.inner));
        painter.rect_stroke(inner_s, 0.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 140, 255, 80)), egui::StrokeKind::Middle);
        // Evidence: ticks and matched labels.
        if let (Some(w), true) = (&work, self.show_overlays) {
            let ev = &w.evidence;
            let tick_col = Color32::from_rgba_unmultiplied(255, 120, 0, 200);
            if let Some([a, _]) = ev.x_axis {
                for &x in &ev.x_ticks {
                    let p = view.to_screen(pos(Pt::new(x, a.y)));
                    painter.line_segment([p - egui::vec2(0.0, 6.0), p + egui::vec2(0.0, 6.0)], Stroke::new(1.5, tick_col));
                }
            }
            if let Some([a, _]) = ev.y_axis {
                for &y in &ev.y_ticks {
                    let p = view.to_screen(pos(Pt::new(a.x, y)));
                    painter.line_segment([p - egui::vec2(6.0, 0.0), p + egui::vec2(6.0, 0.0)], Stroke::new(1.5, tick_col));
                }
            }
            let items = w.text.value.clone().unwrap_or_default();
            for it in items.iter().filter(|i| matches!(i.role, TextRole::XTick | TextRole::YTick)) {
                let r = view.rect_to_screen(to_rect(&it.bbox));
                painter.rect_stroke(r, 1.0, Stroke::new(1.0, widgets::c32(it.role.color()).gamma_multiply(0.7)), egui::StrokeKind::Outside);
            }
        }
        let axes = self.axes_mut().cloned().unwrap_or_else(|| Axes::new(kind, 0.0));
        // Calibrated grid overlay.
        if self.axes_ui.show_grid {
            if let Ok(cal) = axes.calibrate() {
                draw_cal_grid(&painter, view, &cal, &panel.inner);
            }
        }
        // XY: guide lines between point pairs.
        if kind == AxesKind::Xy {
            let s = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
            if axes.points.len() >= 2 {
                painter.line_segment([view.to_screen(pos(axes.points[0])), view.to_screen(pos(axes.points[1]))], s);
            }
            if axes.points.len() >= 4 {
                painter.line_segment([view.to_screen(pos(axes.points[2])), view.to_screen(pos(axes.points[3]))], s);
            }
        }
        let names = kind.point_names();
        for (i, p) in axes.points.iter().enumerate() {
            let label = names.get(i).copied().unwrap_or("?");
            widgets::cal_marker(&painter, view.to_screen(pos(*p)), point_color(kind, i), label, self.axes_ui.selected == Some(i));
        }
        if let (Some(i), Some(h)) = (self.axes_ui.placing, view.hover) {
            if let Some(n) = names.get(i) {
                widgets::label_box(
                    &painter,
                    view.to_screen(h) + egui::vec2(14.0, 14.0),
                    egui::Align2::LEFT_TOP,
                    &format!("click to place {n}"),
                    point_color(kind, i),
                );
            }
        }
        if !interactive {
            return;
        }
        let resp = &view.response;
        let pointer = resp.interact_pointer_pos();
        let nearest = |p: Pos2| -> Option<usize> {
            axes.points
                .iter()
                .enumerate()
                .map(|(i, q)| (i, view.to_screen(pos(*q)).distance(p)))
                .filter(|(_, d)| *d < 12.0)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .map(|(i, _)| i)
        };
        // Pointer: a hand over a calibration point (drag it), a crosshair while placing one.
        if self.axes_ui.drag.is_some() {
            view.set_cursor(egui::CursorIcon::Grabbing);
        } else if resp.hover_pos().and_then(nearest).is_some() {
            view.set_cursor(egui::CursorIcon::Grab);
        } else if self.axes_ui.placing.is_some() {
            view.set_cursor(egui::CursorIcon::Crosshair);
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.axes_ui.drag = pointer.and_then(nearest);
            if self.axes_ui.drag.is_some() {
                self.axes_ui.selected = self.axes_ui.drag;
            }
        }
        if let (Some(i), true) = (self.axes_ui.drag, resp.dragged_by(egui::PointerButton::Primary)) {
            if let Some(p) = pointer {
                let ip = view.to_image(p);
                if let Some(a) = self.axes_mut() {
                    if let Some(q) = a.points.get_mut(i) {
                        *q = pt(ip);
                    }
                }
                self.axes_ui.focus = Some(ip);
                self.edited(Stage::Axes);
            }
        }
        if resp.drag_stopped() {
            self.axes_ui.drag = None;
        }
        if resp.clicked() {
            if let Some(p) = pointer {
                if let Some(i) = self.axes_ui.placing {
                    let ip = pt(view.to_image(p));
                    let total = names.len();
                    if let Some(a) = self.axes_mut() {
                        if i < a.points.len() {
                            a.points[i] = ip;
                        } else {
                            while a.points.len() < i {
                                a.points.push(ip);
                            }
                            a.points.push(ip);
                        }
                        let next = (0..total).find(|k| *k >= a.points.len());
                        self.axes_ui.placing = next;
                        self.axes_ui.selected = Some(i);
                    }
                    self.edited(Stage::Axes);
                    if self.axes_ui.placing.is_none() && kind != AxesKind::Image {
                        self.dialog = Some(Dialog::CalibrationValues);
                    }
                } else {
                    self.axes_ui.selected = nearest(p);
                }
            }
        }
        if let (Some(i), Some(d)) = (self.axes_ui.selected, arrow_nudge(ctx)) {
            if self.dialog.is_none() {
                if let Some(a) = self.axes_mut() {
                    if let Some(q) = a.points.get_mut(i) {
                        q.x += d.x as f64;
                        q.y += d.y as f64;
                        self.axes_ui.focus = Some(pos(*q));
                    }
                }
                self.edited(Stage::Axes);
            }
        }
        if view.hover.is_some() && self.axes_ui.drag.is_none() {
            self.axes_ui.focus = None;
        }
    }
}

/// Faint calibrated grid: lines at nice data values across the panel.
fn draw_cal_grid(painter: &egui::Painter, view: &CanvasView, cal: &Calibration, inner: &plot2csv_core::geom::RectF) {
    let col = Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 0, 200, 110));
    let font = egui::FontId::proportional(13.0);
    let tcol = Color32::from_rgb(170, 0, 170);
    match cal {
        Calibration::Xy(_) => {
            let corners =
                [Pt::new(inner.x0, inner.y0), Pt::new(inner.x1, inner.y0), Pt::new(inner.x0, inner.y1), Pt::new(inner.x1, inner.y1)];
            let d: Vec<Vec<f64>> = corners.iter().map(|p| cal.to_data(*p)).collect();
            let (xlo, xhi) = (d.iter().map(|v| v[0]).fold(f64::MAX, f64::min), d.iter().map(|v| v[0]).fold(f64::MIN, f64::max));
            let (ylo, yhi) = (d.iter().map(|v| v[1]).fold(f64::MAX, f64::min), d.iter().map(|v| v[1]).fold(f64::MIN, f64::max));
            let (lx, ly) = match cal {
                Calibration::Xy(c) => (c.log_x, c.log_y),
                _ => (false, false),
            };
            let ticks = |lo: f64, hi: f64, log: bool| -> Vec<f64> {
                if log && lo > 0.0 {
                    nice_ticks(lo.log10(), hi.log10(), 6).into_iter().map(|e| 10f64.powf(e)).collect()
                } else {
                    nice_ticks(lo, hi, 6)
                }
            };
            for x in ticks(xlo, xhi, lx) {
                if let (Some(a), Some(b)) = (cal.to_pixel(&[x, ylo]), cal.to_pixel(&[x, yhi])) {
                    painter.line_segment([view.to_screen(pos(a)), view.to_screen(pos(b))], col);
                    let lab = cal.format_value(0, x, plot2csv_core::NumFormat::Precision, 4);
                    painter.text(view.to_screen(pos(a)) + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, lab, font.clone(), tcol);
                }
            }
            for y in ticks(ylo, yhi, ly) {
                if let (Some(a), Some(b)) = (cal.to_pixel(&[xlo, y]), cal.to_pixel(&[xhi, y])) {
                    painter.line_segment([view.to_screen(pos(a)), view.to_screen(pos(b))], col);
                    let lab = cal.format_value(1, y, plot2csv_core::NumFormat::Precision, 4);
                    painter.text(view.to_screen(pos(a)) + egui::vec2(2.0, -2.0), egui::Align2::LEFT_BOTTOM, lab, font.clone(), tcol);
                }
            }
        }
        Calibration::Polar(p) => {
            let o = view.to_screen(pos(p.origin));
            for k in 0..12 {
                let th = if p.degrees { k as f64 * 30.0 } else { k as f64 * std::f64::consts::PI / 6.0 };
                let r_far = cal.to_data(Pt::new(inner.x1, inner.y0))[0].max(cal.to_data(Pt::new(inner.x0, inner.y1))[0]);
                if let Some(e) = cal.to_pixel(&[r_far, th]) {
                    painter.line_segment([o, view.to_screen(pos(e))], col);
                }
            }
        }
        Calibration::Ternary(t) => {
            for k in 1..10 {
                let f = k as f64 / 10.0 * if t.range100 { 100.0 } else { 1.0 };
                let rest = if t.range100 { 100.0 } else { 1.0 } - f;
                for (a, b) in [([f, rest, 0.0], [f, 0.0, rest]), ([rest, f, 0.0], [0.0, f, rest]), ([rest, 0.0, f], [0.0, rest, f])] {
                    let pa = view.to_screen(pos(t.to_pixel(a)));
                    let pb = view.to_screen(pos(t.to_pixel(b)));
                    painter.line_segment([pa, pb], col);
                }
            }
        }
        _ => {}
    }
}

/// Inline editor for calibration values (also used by the WPD-style dialog).
pub fn calibration_values_ui(ui: &mut egui::Ui, axes: &mut Axes) -> bool {
    let mut changed = false;
    let te = |ui: &mut egui::Ui, s: &mut String| ui.add(egui::TextEdit::singleline(s).desired_width(80.0)).changed();
    match &mut axes.params {
        AxesParams::Xy(p) => {
            ui.label("Values at the points you clicked:");
            egui::Grid::new("xyvals").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                ui.label("");
                ui.label("Point 1");
                ui.label("Point 2");
                ui.label("Log scale");
                ui.end_row();
                ui.label("X-Axis:");
                changed |= te(ui, &mut p.x1);
                changed |= te(ui, &mut p.x2);
                changed |= ui.checkbox(&mut p.log_x, "").changed();
                ui.end_row();
                ui.label("Y-Axis:");
                changed |= te(ui, &mut p.y1);
                changed |= te(ui, &mut p.y2);
                changed |= ui.checkbox(&mut p.log_y, "").changed();
                ui.end_row();
            });
            ui.label(
                egui::RichText::new("For dates use yyyy/mm/dd or yyyy/mm (e.g. 2013/10/23). For exponents enter 1e-3 for 10⁻³.")
                    .weak()
                    .small(),
            );
            if plot2csv_core::value::parse_date(&p.x1).is_some() || plot2csv_core::value::parse_date(&p.y1).is_some() {
                ui.horizontal(|ui| {
                    ui.label("Date output format");
                    changed |= ui.add(egui::TextEdit::singleline(&mut p.date_format).desired_width(100.0)).changed();
                });
            }
        }
        AxesParams::Bar(p) => {
            egui::Grid::new("barvals").num_columns(2).show(ui, |ui| {
                ui.label("P1 value");
                changed |= te(ui, &mut p.p1);
                ui.end_row();
                ui.label("P2 value");
                changed |= te(ui, &mut p.p2);
                ui.end_row();
            });
            changed |= ui.checkbox(&mut p.log, "Log scale").changed();
        }
        AxesParams::Polar(p) => {
            egui::Grid::new("polvals").num_columns(3).show(ui, |ui| {
                ui.label("");
                ui.label("R");
                ui.label("θ");
                ui.end_row();
                ui.label("Point 1");
                changed |= te(ui, &mut p.r1);
                changed |= te(ui, &mut p.theta1);
                ui.end_row();
                ui.label("Point 2");
                changed |= te(ui, &mut p.r2);
                changed |= te(ui, &mut p.theta2);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut p.degrees, true, "Degrees").changed();
                changed |= ui.radio_value(&mut p.degrees, false, "Radians").changed();
            });
            changed |= ui.checkbox(&mut p.clockwise, "Clockwise").changed();
            changed |= ui.checkbox(&mut p.log_r, "Log scale (r)").changed();
        }
        AxesParams::Ternary(p) => {
            ui.label("Axes orientation");
            ui.horizontal(|ui| {
                changed |=
                    ui.radio_value(&mut p.normal, true, "Normal").on_hover_text("a = 100% at A (bottom-left), b at B, c at C").changed();
                changed |= ui.radio_value(&mut p.normal, false, "Reverse").changed();
            });
            ui.label("Range of variables");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut p.range100, false, "0 to 1").changed();
                changed |= ui.radio_value(&mut p.range100, true, "0 to 100").changed();
            });
        }
        AxesParams::Map(p) => {
            ui.label("Scale size");
            ui.horizontal(|ui| {
                changed |= te(ui, &mut p.length);
                changed |= ui.add(egui::TextEdit::singleline(&mut p.units).desired_width(70.0)).changed();
            });
        }
        AxesParams::Image => {
            ui.label(egui::RichText::new("Image axes: values are pixel coordinates.").weak());
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice() {
        assert_eq!(nice_ticks(0.0, 6.0, 6), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(nice_ticks(-1.0, 1.0, 4), vec![-1.0, -0.5, 0.0, 0.5, 1.0]);
    }
}
