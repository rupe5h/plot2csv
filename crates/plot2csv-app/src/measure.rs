//! Analyze menu: distance and angle measurements on the corrected image.

use egui::{Color32, Stroke};
use plot2csv_core::axes::angle_deg;
use plot2csv_core::geom::Pt;

use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::stages::{pos, pt};
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeasureMode {
    Distance,
    Angle,
}

#[derive(Default)]
pub struct MeasureUi {
    pub active: Option<MeasureMode>,
    pub deleting: bool,
    pending: Vec<Pt>,
}

impl MeasureUi {
    pub fn start(&mut self, m: MeasureMode) {
        self.active = Some(m);
        self.deleting = false;
        self.pending.clear();
    }

    /// Drop the half-made measurement (after undo/redo).
    pub fn clear_transient(&mut self) {
        self.pending.clear();
    }
}

const COL: Color32 = Color32::from_rgb(220, 20, 20);

impl Plot2CsvApp {
    fn measure_label(&self, i: usize, d: &[Pt; 2]) -> String {
        match self.active_calibration() {
            Some(cal) => {
                let (v, u) = cal.distance(d[0], d[1]);
                format!("[{i}]: {v:.2} {u}")
            }
            None => format!("[{i}]: {:.2} px", d[0].dist(d[1])),
        }
    }

    pub fn measure_sidebar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let Some(mode) = self.measure.active else { return };
        let title = if mode == MeasureMode::Distance { "Measure Distances" } else { "Measure Angles" };
        widgets::section(ui, title);
        ui.horizontal_wrapped(|ui| {
            let add = if mode == MeasureMode::Distance { "Add Pair (A)" } else { "Add Angle (A)" };
            let del = if mode == MeasureMode::Distance { "Delete Pair (D)" } else { "Delete Angle (D)" };
            if ui.selectable_label(!self.measure.deleting, add).clicked() {
                self.measure.deleting = false;
            }
            if ui.selectable_label(self.measure.deleting, del).clicked() {
                self.measure.deleting = true;
            }
            if ui.button("Clear All").clicked() {
                if let Some(id) = self.project.active_panel {
                    let m = &mut self.project.work_mut(id).measurements;
                    match mode {
                        MeasureMode::Distance => m.distances.clear(),
                        MeasureMode::Angle => m.angles.clear(),
                    }
                }
            }
            if ui.button("Close").clicked() {
                self.measure.active = None;
            }
        });
        let hint = match (mode, self.measure.deleting) {
            (_, true) => "Click a measurement to delete it.",
            (MeasureMode::Distance, _) => "Click two points.",
            (MeasureMode::Angle, _) => "Click three points: end, vertex, end.",
        };
        ui.label(egui::RichText::new(hint).weak());
        let m = self.project.active_work().map(|w| w.measurements.clone()).unwrap_or_default();
        let mut text = String::new();
        match mode {
            MeasureMode::Distance => {
                for (i, d) in m.distances.iter().enumerate() {
                    text += &self.measure_label(i, d);
                    text.push('\n');
                }
            }
            MeasureMode::Angle => {
                for (i, a) in m.angles.iter().enumerate() {
                    text += &format!("[{i}]: {:.2}°\n", angle_deg(a[0], a[1], a[2]));
                }
            }
        }
        if !text.is_empty() {
            ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_rows(4));
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(text.clone());
            }
        }
    }

    pub fn measure_canvas(&mut self, ui: &mut egui::Ui, view: &CanvasView) {
        let Some(mode) = self.measure.active else { return };
        let Some(id) = self.project.active_panel else { return };
        let painter = ui.painter_at(view.rect);
        let m = self.project.work(id).map(|w| w.measurements.clone()).unwrap_or_default();
        let s = Stroke::new(2.0, COL);
        for (i, d) in m.distances.iter().enumerate() {
            let (a, b) = (view.to_screen(pos(d[0])), view.to_screen(pos(d[1])));
            painter.line_segment([a, b], s);
            painter.circle_filled(a, 3.0, COL);
            painter.circle_filled(b, 3.0, COL);
            widgets::label_box(&painter, a.lerp(b, 0.5), egui::Align2::CENTER_BOTTOM, &self.measure_label(i, d), COL);
        }
        for (i, a) in m.angles.iter().enumerate() {
            let p: Vec<_> = a.iter().map(|q| view.to_screen(pos(*q))).collect();
            painter.line_segment([p[0], p[1]], s);
            painter.line_segment([p[1], p[2]], s);
            widgets::label_box(&painter, p[1], egui::Align2::LEFT_TOP, &format!("[{i}]: {:.2}°", angle_deg(a[0], a[1], a[2])), COL);
        }
        for q in &self.measure.pending {
            painter.circle_filled(view.to_screen(pos(*q)), 3.5, COL);
        }
        if let (Some(last), Some(h)) = (self.measure.pending.last(), view.hover) {
            painter.line_segment([view.to_screen(pos(*last)), view.to_screen(h)], Stroke::new(1.0, COL));
        }
        view.set_cursor(egui::CursorIcon::Crosshair);
        let ctx = ui.ctx().clone();
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::A)) {
                self.measure.deleting = false;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::D)) {
                self.measure.deleting = true;
            }
        }
        if !view.response.clicked() {
            return;
        }
        let Some(p) = view.response.interact_pointer_pos().map(|p| pt(view.to_image(p))) else { return };
        let meas = &mut self.project.work_mut(id).measurements;
        if self.measure.deleting {
            let tol = 8.0 / view.zoom as f64;
            match mode {
                MeasureMode::Distance => {
                    if let Some(i) = meas.distances.iter().position(|d| plot2csv_core::geom::point_segment_dist(p, d[0], d[1]) < tol) {
                        meas.distances.remove(i);
                    }
                }
                MeasureMode::Angle => {
                    if let Some(i) = meas.angles.iter().position(|a| {
                        plot2csv_core::geom::point_segment_dist(p, a[0], a[1]) < tol || plot2csv_core::geom::point_segment_dist(p, a[1], a[2]) < tol
                    }) {
                        meas.angles.remove(i);
                    }
                }
            }
            return;
        }
        self.measure.pending.push(p);
        let need = if mode == MeasureMode::Distance { 2 } else { 3 };
        if self.measure.pending.len() == need {
            let v = std::mem::take(&mut self.measure.pending);
            match mode {
                MeasureMode::Distance => meas.distances.push([v[0], v[1]]),
                MeasureMode::Angle => meas.angles.push([v[0], v[1], v[2]]),
            }
        }
    }
}
