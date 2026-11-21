//! ③ Plot area: one or more panels with an inner (data) box and an outer (labels) box.

use egui::{Color32, CursorIcon, Pos2, Rect, Stroke, StrokeKind};
use plot2csv_core::axes::AxesKind;
use plot2csv_core::geom::RectF;
use plot2csv_core::project::{Panel, Stage};

use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::icons::{self, Icon};
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Inner,
    Outer,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    Move,
    L,
    R,
    T,
    B,
    TL,
    TR,
    BL,
    BR,
}

#[derive(Default)]
pub struct RegionUi {
    pub selected: Option<u32>,
    drag: Option<(u32, Which, Handle)>,
    pub drawing: bool,
    draw_start: Option<Pos2>,
    draw_cur: Option<Pos2>,
}

impl RegionUi {
    /// Drop the selection, drag and a half-drawn box (after undo/redo).
    pub fn clear_transient(&mut self) {
        self.selected = None;
        self.drag = None;
        self.draw_start = None;
        self.draw_cur = None;
    }
}

pub fn to_rect(r: &RectF) -> Rect {
    Rect::from_min_max(Pos2::new(r.x0 as f32, r.y0 as f32), Pos2::new(r.x1 as f32, r.y1 as f32))
}

fn hit(r: Rect, p: Pos2, tol: f32) -> Option<Handle> {
    let near = |a: f32, b: f32| (a - b).abs() < tol;
    let inside_x = p.x > r.left() - tol && p.x < r.right() + tol;
    let inside_y = p.y > r.top() - tol && p.y < r.bottom() + tol;
    if !(inside_x && inside_y) {
        return None;
    }
    let (l, rr, t, b) = (near(p.x, r.left()), near(p.x, r.right()), near(p.y, r.top()), near(p.y, r.bottom()));
    Some(match (l, rr, t, b) {
        (true, _, true, _) => Handle::TL,
        (_, true, true, _) => Handle::TR,
        (true, _, _, true) => Handle::BL,
        (_, true, _, true) => Handle::BR,
        (true, _, _, _) => Handle::L,
        (_, true, _, _) => Handle::R,
        (_, _, true, _) => Handle::T,
        (_, _, _, true) => Handle::B,
        _ => Handle::Move,
    })
}

/// Pointer that tells what dragging a handle does.
fn handle_cursor(h: Handle, dragging: bool) -> CursorIcon {
    match h {
        Handle::Move if dragging => CursorIcon::Grabbing,
        Handle::Move => CursorIcon::Grab,
        Handle::L | Handle::R => CursorIcon::ResizeHorizontal,
        Handle::T | Handle::B => CursorIcon::ResizeVertical,
        Handle::TL | Handle::BR => CursorIcon::ResizeNwSe,
        Handle::TR | Handle::BL => CursorIcon::ResizeNeSw,
    }
}

fn apply_handle(r: &mut RectF, h: Handle, d: (f64, f64)) {
    match h {
        Handle::Move => {
            r.x0 += d.0;
            r.x1 += d.0;
            r.y0 += d.1;
            r.y1 += d.1;
        }
        Handle::L => r.x0 += d.0,
        Handle::R => r.x1 += d.0,
        Handle::T => r.y0 += d.1,
        Handle::B => r.y1 += d.1,
        Handle::TL => {
            r.x0 += d.0;
            r.y0 += d.1;
        }
        Handle::TR => {
            r.x1 += d.0;
            r.y0 += d.1;
        }
        Handle::BL => {
            r.x0 += d.0;
            r.y1 += d.1;
        }
        Handle::BR => {
            r.x1 += d.0;
            r.y1 += d.1;
        }
    }
    *r = RectF::new(r.x0, r.y0, r.x1, r.y1);
}

pub const PANEL_COLOR: Color32 = Color32::from_rgb(0, 140, 255);

impl Plot2CsvApp {
    pub fn auto_regions(&mut self, _ctx: &egui::Context) {
        let Some(g) = self.imgs.corrected_gray.clone() else { return };
        let res = plot2csv_vision::regions::detect_panels(&g);
        let panels: Vec<Panel> = res
            .panels
            .iter()
            .enumerate()
            .map(|(i, p)| Panel {
                id: i as u32 + 1,
                name: format!("Plot {}", i + 1),
                kind: p.kind,
                inner: p.inner,
                outer: p.outer,
                enabled: true,
            })
            .collect();
        self.region_ui = RegionUi { selected: panels.first().map(|p| p.id), ..Default::default() };
        self.project.regions.propose(panels, res.note);
    }

    fn panels_mut(&mut self) -> &mut Vec<Panel> {
        if self.project.regions.value.is_none() {
            self.project.regions.value = Some(Vec::new());
        }
        self.project.regions.value.as_mut().unwrap()
    }

    pub fn region_sidebar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        widgets::section(ui, "③ Plot areas");
        if !self.project.regions.note.is_empty() {
            ui.label(egui::RichText::new(&self.project.regions.note).color(widgets::INFO));
        }
        ui.label(
            egui::RichText::new("Blue box: the data area inside the axes. Grey dashed box: data area plus tick labels and titles (used for text reading). Drag edges to adjust.")
                .weak(),
        );
        let mut changed = false;
        let mut delete = None;
        let selected = self.region_ui.selected;
        let mut new_sel = selected;
        let panels = self.panels_mut();
        for p in panels.iter_mut() {
            let sel = selected == Some(p.id);
            egui::Frame::group(ui.style())
                .fill(if sel { ui.visuals().selection.bg_fill.gamma_multiply(0.35) } else { Color32::TRANSPARENT })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut p.enabled, "").on_hover_text("Include this panel").changed();
                        let r = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(110.0));
                        changed |= r.changed();
                        if r.gained_focus() {
                            new_sel = Some(p.id);
                        }
                        if ui.small_button(icons::image(ui, Icon::TrashCan)).on_hover_text("Delete this panel").clicked() {
                            delete = Some(p.id);
                        }
                        if ui.selectable_label(sel, "select").clicked() {
                            new_sel = Some(p.id);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Type");
                        egui::ComboBox::from_id_salt(("kind", p.id)).selected_text(p.kind.label()).show_ui(ui, |ui| {
                            for k in AxesKind::ALL {
                                changed |= ui.selectable_value(&mut p.kind, k, k.label()).changed();
                            }
                        });
                    });
                    if sel {
                        ui.label(
                            egui::RichText::new(format!(
                                "Data area: {:.0},{:.0} – {:.0},{:.0}",
                                p.inner.x0, p.inner.y0, p.inner.x1, p.inner.y1
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });
        }
        if let Some(id) = delete {
            self.panels_mut().retain(|p| p.id != id);
            changed = true;
            if new_sel == Some(id) {
                new_sel = None;
            }
        }
        self.region_ui.selected = new_sel;
        ui.add_space(4.0);
        let draw_label = if self.region_ui.drawing {
            egui::Atoms::new("Drawing… (drag on the image)")
        } else {
            egui::Atoms::new((icons::image(ui, Icon::Plus), "Draw a new plot area"))
        };
        if ui.selectable_label(self.region_ui.drawing, draw_label).clicked() {
            self.region_ui.drawing = !self.region_ui.drawing;
        }
        if ui.button("Use the whole image as one plot").clicked() {
            let (w, h) = (self.imgs.corrected_size().x as f64, self.imgs.corrected_size().y as f64);
            let id = self.project.next_panel_id();
            let p = Panel {
                id,
                name: format!("Plot {id}"),
                kind: AxesKind::Xy,
                inner: RectF::new(0.0, 0.0, w, h),
                outer: RectF::new(0.0, 0.0, w, h),
                enabled: true,
            };
            *self.panels_mut() = vec![p];
            self.region_ui.selected = Some(id);
            changed = true;
        }
        if changed {
            self.edited(Stage::Region);
        }
    }

    pub fn region_canvas(&mut self, ui: &mut egui::Ui, view: &CanvasView, interactive: bool) {
        let painter = ui.painter_at(view.rect);
        let panels = self.project.panel_list().to_vec();
        for p in &panels {
            let outer = view.rect_to_screen(to_rect(&p.outer));
            let inner = view.rect_to_screen(to_rect(&p.inner));
            let alpha = if p.enabled { 255 } else { 90 };
            let dash = egui::Shape::dashed_line(
                &[outer.left_top(), outer.right_top(), outer.right_bottom(), outer.left_bottom(), outer.left_top()],
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(110, 110, 110, alpha)),
                6.0,
                4.0,
            );
            painter.extend(dash);
            let col = Color32::from_rgba_unmultiplied(PANEL_COLOR.r(), PANEL_COLOR.g(), PANEL_COLOR.b(), alpha);
            painter.rect_filled(inner, 0.0, Color32::from_rgba_unmultiplied(0, 140, 255, if p.enabled { 18 } else { 6 }));
            painter.rect_stroke(inner, 0.0, Stroke::new(2.0, col), StrokeKind::Middle);
            widgets::label_box(
                &painter,
                inner.left_top() + egui::vec2(4.0, 4.0),
                egui::Align2::LEFT_TOP,
                &format!("{} · {}", p.name, p.kind.label()),
                col,
            );
            if self.region_ui.selected == Some(p.id) {
                for r in [inner, outer] {
                    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                        painter.rect_filled(Rect::from_center_size(c, egui::vec2(8.0, 8.0)), 1.0, col);
                    }
                }
            }
        }
        if let (Some(a), Some(b)) = (self.region_ui.draw_start, self.region_ui.draw_cur) {
            let r = Rect::from_two_pos(view.to_screen(a), view.to_screen(b));
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, PANEL_COLOR), StrokeKind::Middle);
        }
        if !interactive {
            return;
        }
        let resp = &view.response;
        let pointer = resp.interact_pointer_pos();
        if self.region_ui.drawing {
            view.set_cursor(CursorIcon::Crosshair);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.region_ui.draw_start = pointer.map(|p| view.to_image(p));
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                self.region_ui.draw_cur = pointer.map(|p| view.to_image(p));
            }
            if resp.drag_stopped() {
                if let (Some(a), Some(b)) = (self.region_ui.draw_start.take(), self.region_ui.draw_cur.take()) {
                    let inner = RectF::new(a.x as f64, a.y as f64, b.x as f64, b.y as f64);
                    if inner.width() > 10.0 && inner.height() > 10.0 {
                        let (w, h) = (self.imgs.corrected_size().x as f64, self.imgs.corrected_size().y as f64);
                        let outer = RectF::new(
                            inner.x0 - 0.15 * inner.width(),
                            inner.y0 - 0.08 * inner.height(),
                            inner.x1 + 0.05 * inner.width(),
                            inner.y1 + 0.2 * inner.height(),
                        )
                        .clamp_to(w, h);
                        let id = self.project.next_panel_id();
                        self.panels_mut().push(Panel { id, name: format!("Plot {id}"), kind: AxesKind::Xy, inner, outer, enabled: true });
                        self.region_ui.selected = Some(id);
                        self.region_ui.drawing = false;
                        self.edited(Stage::Region);
                    }
                }
            }
            return;
        }
        // The handle under a screen position: selected panel first, inner before outer.
        let selected = self.region_ui.selected;
        let handle_at = |p: Pos2| -> Option<(u32, Which, Handle)> {
            let mut order: Vec<&Panel> = panels.iter().collect();
            order.sort_by_key(|q| if Some(q.id) == selected { 0 } else { 1 });
            for q in order {
                for which in [Which::Inner, Which::Outer] {
                    let r = if which == Which::Inner { &q.inner } else { &q.outer };
                    match hit(view.rect_to_screen(to_rect(r)), p, 8.0) {
                        Some(Handle::Move) if which == Which::Outer => {}
                        Some(h) => return Some((q.id, which, h)),
                        None => {}
                    }
                }
            }
            None
        };
        // Pointer: what a drag from here does (resize an edge or corner, move the panel).
        match self.region_ui.drag {
            Some((_, _, h)) => view.set_cursor(handle_cursor(h, true)),
            None => {
                if let Some((_, _, h)) = resp.hover_pos().and_then(handle_at) {
                    view.set_cursor(handle_cursor(h, false));
                }
            }
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some((id, which, h)) = pointer.and_then(handle_at) {
                self.region_ui.drag = Some((id, which, h));
                self.region_ui.selected = Some(id);
            }
        }
        if let Some((id, which, h)) = self.region_ui.drag {
            if resp.dragged_by(egui::PointerButton::Primary) {
                let d = resp.drag_delta() / view.zoom;
                let d = (d.x as f64, d.y as f64);
                if let Some(p) = self.panels_mut().iter_mut().find(|p| p.id == id) {
                    if h == Handle::Move {
                        apply_handle(&mut p.inner, h, d);
                        apply_handle(&mut p.outer, h, d);
                    } else if which == Which::Inner {
                        apply_handle(&mut p.inner, h, d);
                        // keep the outer box around the inner one
                        p.outer = RectF::new(
                            p.outer.x0.min(p.inner.x0),
                            p.outer.y0.min(p.inner.y0),
                            p.outer.x1.max(p.inner.x1),
                            p.outer.y1.max(p.inner.y1),
                        );
                    } else {
                        apply_handle(&mut p.outer, h, d);
                    }
                }
                self.edited(Stage::Region);
            }
            if resp.drag_stopped() {
                self.region_ui.drag = None;
            }
        }
        if resp.clicked() {
            if let Some(p) = pointer {
                let ip = view.to_image(p);
                self.region_ui.selected = panels.iter().find(|q| q.inner.contains(super::pt(ip))).map(|q| q.id).or(self.region_ui.selected);
            }
        }
    }
}
