//! ② Geometry correction: rotation + perspective, reviewed with draggable corners.

use egui::{Color32, Pos2, Stroke};
use plot2csv_core::geom::{Homography, Pt};
use plot2csv_core::project::{Correction, Stage};

use super::{arrow_nudge, pos, pt};
use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::icons::{self, Icon};
use crate::widgets;

#[derive(Default)]
pub struct CorrectUi {
    /// Show the corrected result instead of the source with handles.
    pub preview: bool,
    drag: Option<usize>,
    selected: Option<usize>,
    pub focus: Option<Pos2>,
    rotation: f32,
}

impl CorrectUi {
    pub fn editing_quad(&self) -> bool {
        !self.preview
    }

    /// Drop the drag and selection (after undo/redo).
    pub fn clear_transient(&mut self) {
        self.drag = None;
        self.selected = None;
    }
}

fn full_quad(w: f64, h: f64) -> [Pt; 4] {
    [Pt::new(0.0, 0.0), Pt::new(w, 0.0), Pt::new(w, h), Pt::new(0.0, h)]
}

impl Plot2CsvApp {
    pub fn auto_correct(&mut self, _ctx: &egui::Context) {
        let Some(img) = self.imgs.source.clone() else { return };
        let gray = plot2csv_vision::raster::rgba_to_gray(&img);
        let det = plot2csv_vision::correct::detect_correction(&gray);
        let note = det.note.clone();
        self.project.correction.propose(det.correction, note);
        self.correct_ui = CorrectUi::default();
        self.correct_ui.rotation = ((det.rotation_deg * 10.0).round() / 10.0) as f32;
    }

    fn correction_mut(&mut self) -> &mut Correction {
        if self.project.correction.value.is_none() {
            let (w, h) = self.imgs.source.as_ref().map(|i| (i.width(), i.height())).unwrap_or((1, 1));
            self.project.correction.value = Some(Correction::identity(w, h));
        }
        self.project.correction.value.as_mut().unwrap()
    }

    pub fn correct_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        widgets::section(ui, "② Rotation & perspective");
        let note = self.project.correction.note.clone();
        if !note.is_empty() {
            ui.label(egui::RichText::new(note).color(widgets::INFO));
        }
        let Some(src) = self.imgs.source.clone() else { return };
        let (w, h) = (src.width() as f64, src.height() as f64);
        let mut corr = self.correction_mut().clone();
        let mut changed = false;

        changed |= ui.checkbox(&mut corr.enabled, "Apply correction").changed();
        ui.add_enabled_ui(corr.enabled, |ui| {
            ui.label(
                egui::RichText::new("Drag the four corners onto the plot frame (or any rectangle that should be square to the page).")
                    .weak(),
            );
            ui.horizontal(|ui| {
                ui.label("Rotate");
                let r = ui.add(egui::Slider::new(&mut self.correct_ui.rotation, -45.0..=45.0).suffix("°").step_by(0.1));
                // Only a real interaction counts (the slider may round the value on first show).
                let touched = r.dragged() || r.has_focus() || r.clicked() || r.lost_focus();
                if r.changed() && touched {
                    // Rotate the full-image quad about the centre; content rotates the other way.
                    let c = Pt::new(w / 2.0, h / 2.0);
                    let rot = Homography::rotation_about((self.correct_ui.rotation as f64).to_radians(), c);
                    let q = full_quad(w, h).map(|p| rot.apply(p));
                    corr.quad = q;
                    let (ow, oh) = Correction::natural_size(&q);
                    corr.out_w = ow;
                    corr.out_h = oh;
                    corr.method = "manual rotation".into();
                    changed = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button((icons::image(ui, Icon::RotateRight), "Rotate 90°")).clicked() {
                    corr.rotate90();
                    changed = true;
                }
                if ui.button("Reset corners").clicked() {
                    corr.quad = full_quad(w, h);
                    corr.out_w = w as u32;
                    corr.out_h = h as u32;
                    self.correct_ui.rotation = 0.0;
                    changed = true;
                }
                if ui.button("Fit output to corners").on_hover_text("Recompute the output size from the corner positions").clicked() {
                    let (ow, oh) = Correction::natural_size(&corr.quad);
                    corr.out_w = ow;
                    corr.out_h = oh;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Output size");
                changed |= ui.add(egui::DragValue::new(&mut corr.out_w).range(16..=8000)).changed();
                ui.label("×");
                changed |= ui.add(egui::DragValue::new(&mut corr.out_h).range(16..=8000)).changed();
            });
        });
        ui.add_space(6.0);
        let label = if self.correct_ui.preview {
            egui::Atoms::new((icons::image(ui, Icon::ChevronLeft), "Back to editing corners"))
        } else {
            egui::Atoms::new((icons::image(ui, Icon::Eye), "Preview result"))
        };
        if ui.add_enabled(corr.enabled, egui::Button::new(label)).clicked() {
            self.correct_ui.preview = !self.correct_ui.preview;
            if self.correct_ui.preview {
                self.defer("Applying correction…", |app, ctx| {
                    app.imgs.sync_corrected(ctx, &app.project);
                    app.canvas.fit();
                });
            }
            self.canvas.fit();
        }
        if !corr.enabled {
            self.correct_ui.preview = false;
        }
        if changed {
            if let Some(v) = self.project.correction.value.as_mut() {
                *v = corr;
            }
            self.edited(Stage::Correct);
            if self.correct_ui.preview {
                self.defer("Applying correction…", |app, ctx| {
                    app.imgs.sync_corrected(ctx, &app.project);
                });
            }
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Tip: arrow keys nudge the selected corner (Shift = 10 px). Right-drag or middle-drag pans.")
                .weak()
                .small(),
        );
        let _ = ctx;
    }

    pub fn correct_canvas(&mut self, ui: &mut egui::Ui, view: &CanvasView, interactive: bool) {
        if self.correct_ui.preview {
            return;
        }
        let Some(corr) = self.project.correction.value.clone() else { return };
        if !corr.enabled {
            return;
        }
        let painter = ui.painter_at(view.rect);
        let q = corr.quad;
        let sq: Vec<Pos2> = q.iter().map(|p| view.to_screen(pos(*p))).collect();
        // Projective grid inside the quad.
        let inv = corr.homography().inverse().unwrap_or_default();
        let (ow, oh) = (corr.out_w as f64, corr.out_h as f64);
        let grid = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 160, 255, 90));
        for i in 1..10 {
            let t = i as f64 / 10.0;
            let a = inv.apply(Pt::new(ow * t, 0.0));
            let b = inv.apply(Pt::new(ow * t, oh));
            painter.line_segment([view.to_screen(pos(a)), view.to_screen(pos(b))], grid);
            let a = inv.apply(Pt::new(0.0, oh * t));
            let b = inv.apply(Pt::new(ow, oh * t));
            painter.line_segment([view.to_screen(pos(a)), view.to_screen(pos(b))], grid);
        }
        let edge = Stroke::new(2.0, Color32::from_rgb(0, 140, 255));
        for i in 0..4 {
            painter.line_segment([sq[i], sq[(i + 1) % 4]], edge);
        }
        for (i, s) in sq.iter().enumerate() {
            let sel = self.correct_ui.selected == Some(i);
            widgets::marker(&painter, *s, Color32::from_rgb(0, 140, 255), 6.0, Some(["TL", "TR", "BR", "BL"][i]), sel);
        }
        if !interactive {
            return;
        }
        let r = &view.response;
        // Pointer: a hand over a corner that can be dragged.
        if self.correct_ui.drag.is_some() {
            view.set_cursor(egui::CursorIcon::Grabbing);
        } else if r.hover_pos().is_some_and(|p| sq.iter().any(|s| s.distance(p) < 14.0)) {
            view.set_cursor(egui::CursorIcon::Grab);
        }
        if r.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = r.interact_pointer_pos() {
                self.correct_ui.drag = sq
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (i, s.distance(p)))
                    .filter(|(_, d)| *d < 14.0)
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    .map(|(i, _)| i);
                self.correct_ui.selected = self.correct_ui.drag.or(self.correct_ui.selected);
            }
        }
        if let (Some(i), true) = (self.correct_ui.drag, r.dragged_by(egui::PointerButton::Primary)) {
            if let Some(p) = r.interact_pointer_pos() {
                let ip = view.to_image(p);
                if let Some(c) = self.project.correction.value.as_mut() {
                    c.quad[i] = pt(ip);
                    c.method = "manual".into();
                }
                self.correct_ui.focus = Some(ip);
                self.edited(Stage::Correct);
            }
        }
        if r.drag_stopped() {
            self.correct_ui.drag = None;
        }
        if r.clicked() {
            if let Some(p) = r.interact_pointer_pos() {
                self.correct_ui.selected = sq
                    .iter()
                    .enumerate()
                    .map(|(i, s)| (i, s.distance(p)))
                    .filter(|(_, d)| *d < 14.0)
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    .map(|(i, _)| i);
            }
        }
        if let (Some(i), Some(d)) = (self.correct_ui.selected, arrow_nudge(ui.ctx())) {
            if let Some(c) = self.project.correction.value.as_mut() {
                c.quad[i].x += d.x as f64;
                c.quad[i].y += d.y as f64;
                self.correct_ui.focus = Some(pos(c.quad[i]));
            }
            self.edited(Stage::Correct);
        }
        if view.hover.is_some() && self.correct_ui.drag.is_none() {
            self.correct_ui.focus = None;
        }
    }
}
