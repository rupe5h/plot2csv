//! ④ Text: OCR of tick labels, titles and legend entries, with review/editing.

use egui::{Color32, CursorIcon, Pos2, Rect, Stroke, StrokeKind};
use plot2csv_core::geom::{Pt, RectF};
use plot2csv_core::project::{Stage, TextItem, TextRole};
use plot2csv_ocr::labels::{classify, label_value};
use plot2csv_ocr::panel::{crop, map_lines, merge_items, panel_crops, Crop};
use plot2csv_ocr::powers::{apply_power_labels, find_power_labels, PowerScan};
use plot2csv_ocr::TextLine;

use super::region::to_rect;
use crate::app::{OcrJob, Plot2CsvApp};
use crate::canvas::CanvasView;
use crate::icons::{self, Icon};
use crate::platform;
use crate::widgets::{self, c32};

#[derive(Default)]
pub struct TextUi {
    pub selected: Option<usize>,
    pub drawing: bool,
    draw_start: Option<Pos2>,
    draw_cur: Option<Pos2>,
    started_at: Option<f64>,
    /// Items from the first OCR pass while tick-guided OCR runs: (items, is-x flags of the extra boxes).
    pending: Option<(Vec<TextItem>, Vec<bool>)>,
    /// Items while the exponents of power labels (10², 10⁻¹) are read.
    pending_powers: Option<(Vec<TextItem>, PowerScan)>,
    /// A box's text being edited in place on the image (double-click).
    inline: Option<InlineEdit>,
    /// The selection was made on the image: bring its row into view.
    scroll_to_selected: bool,
}

/// In-place editor for one text box.
struct InlineEdit {
    index: usize,
    text: String,
    /// Keyboard focus was handed to the editor.
    focused: bool,
}

impl TextUi {
    /// Drop the selection and a half-drawn box (after undo/redo).
    pub fn clear_transient(&mut self) {
        self.selected = None;
        self.draw_start = None;
        self.draw_cur = None;
        self.inline = None;
    }
}

/// Fill of the editable Text cells, and of the read-only Value cells.
fn text_cell_colors(ui: &egui::Ui) -> (Color32, Color32) {
    if ui.visuals().dark_mode {
        (Color32::from_gray(12), Color32::from_gray(58))
    } else {
        (Color32::WHITE, Color32::from_gray(222))
    }
}

/// What an OCR job is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OcrPurpose {
    /// Whole panel (stage auto step).
    Auto,
    /// A box drawn by the user: add what is found.
    Add,
    /// Re-read one existing item.
    Replace(usize),
    /// Second auto pass: tick labels the detector missed.
    AutoTicks,
    /// Last auto pass: exponents of tick labels written as powers.
    AutoPowers,
}

/// One crop to OCR (optionally with fixed line boxes: recognise only).
pub struct OcrTask {
    pub crop: Crop,
    pub boxes: Option<Vec<[f32; 4]>>,
}

impl OcrTask {
    fn new(offset: Pt, img: image::GrayImage, rotated_from_h: Option<f64>, boxes: Option<Vec<[f32; 4]>>) -> Self {
        Self { crop: Crop { offset, img, rotated_from_h }, boxes }
    }
}

impl Plot2CsvApp {
    pub fn auto_text(&mut self, ctx: &egui::Context) {
        let Some(id) = self.project.active_panel else { return };
        let Some(panel) = self.project.panel(id).cloned() else { return };
        let Some(g) = self.imgs.corrected_gray.clone() else {
            // Corrected image not ready yet: try again after it is built.
            self.defer("Preparing image…", |app, ctx| {
                app.imgs.sync_corrected(ctx, &app.project);
                if app.imgs.corrected_gray.is_some() {
                    app.auto_text(ctx);
                }
            });
            return;
        };
        if !platform::ocr_available() {
            self.project.work_mut(id).text.propose(Vec::new(), "OCR is not available in this build; add text manually or skip.");
            return;
        }
        let tasks: Vec<OcrTask> = panel_crops(&g, &panel).into_iter().map(|crop| OcrTask { crop, boxes: None }).collect();
        self.text_ui.started_at = Some(ctx.input(|i| i.time));
        self.start_ocr(ctx, id, OcrPurpose::Auto, tasks);
    }

    pub fn start_ocr(&mut self, ctx: &egui::Context, panel_id: u32, purpose: OcrPurpose, mut tasks: Vec<OcrTask>) {
        if tasks.is_empty() {
            return;
        }
        let first = tasks.remove(0);
        let rx = platform::run_ocr(ctx, first.crop.img.clone(), first.boxes.clone());
        self.ocr_job = Some(OcrJob { panel_id, purpose, current: first, rx, queue: tasks, collected: Vec::new() });
    }

    pub fn poll_ocr(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.ocr_job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("OCR job stopped".into()),
        };
        match res {
            Ok(lines) => {
                let mapped = map_lines(lines, &job.current.crop);
                job.collected.extend(mapped);
            }
            Err(e) => {
                self.status = format!("OCR failed: {e}");
                let id = job.panel_id;
                let purpose = job.purpose;
                self.ocr_job = None;
                // A failed follow-up pass keeps what the earlier passes read.
                let earlier = match purpose {
                    OcrPurpose::Auto => Some(Vec::new()),
                    OcrPurpose::AutoTicks => self.text_ui.pending.take().map(|p| p.0),
                    OcrPurpose::AutoPowers => self.text_ui.pending_powers.take().map(|p| p.0),
                    _ => None,
                };
                if let Some(items) = earlier {
                    self.project.work_mut(id).text.propose(items, format!("OCR failed: {e}"));
                }
                return;
            }
        }
        if !job.queue.is_empty() {
            let next = job.queue.remove(0);
            job.rx = platform::run_ocr(ctx, next.crop.img.clone(), next.boxes.clone());
            job.current = next;
            return;
        }
        let job = self.ocr_job.take().unwrap();
        self.finish_ocr(ctx, job);
    }

    fn finish_ocr(&mut self, ctx: &egui::Context, job: OcrJob) {
        let Some(panel) = self.project.panel(job.panel_id).cloned() else { return };
        let dt = self.text_ui.started_at.map(|t| ctx.input(|i| i.time) - t).unwrap_or(0.0);
        let lines: Vec<(TextLine, bool)> = job.collected.clone();
        let new_items = if matches!(job.purpose, OcrPurpose::Replace(_) | OcrPurpose::AutoTicks | OcrPurpose::AutoPowers) {
            Vec::new()
        } else {
            merge_items(job.collected, &panel.inner)
        };
        // After the first pass, read tick labels the line detector missed.
        if job.purpose == OcrPurpose::Auto {
            if let Some(g) = self.imgs.corrected_gray.clone() {
                let extra = plot2csv_ocr::panel::missing_tick_label_boxes(&g, &panel, &new_items);
                if !extra.is_empty() {
                    let boxes: Vec<[f32; 4]> = extra.iter().map(|e| e.0).collect();
                    let flags: Vec<bool> = extra.iter().map(|e| e.1).collect();
                    self.text_ui.pending = Some((new_items, flags));
                    let task = OcrTask::new(Pt::new(0.0, 0.0), (*g).clone(), None, Some(boxes));
                    self.start_ocr(ctx, job.panel_id, OcrPurpose::AutoTicks, vec![task]);
                    return;
                }
            }
        }
        let (new_items, purpose) = match job.purpose {
            OcrPurpose::AutoTicks => {
                let (mut items, flags) = self.text_ui.pending.take().unwrap_or_default();
                let tagged: Vec<(TextLine, bool)> = lines.iter().map(|(l, _)| l.clone()).zip(flags).collect();
                items.extend(plot2csv_ocr::panel::tick_items(&tagged));
                (items, OcrPurpose::Auto)
            }
            OcrPurpose::AutoPowers => {
                let Some((mut items, scan)) = self.text_ui.pending_powers.take() else { return };
                let read: Vec<TextLine> = lines.iter().map(|(l, _)| l.clone()).collect();
                apply_power_labels(&mut items, &scan, &read, &panel.inner);
                (items, OcrPurpose::Auto)
            }
            p => (new_items, p),
        };
        // Tick labels written as powers (10², 10⁻¹): the line OCR drops their
        // exponents, so read those on their own before proposing.
        if matches!(job.purpose, OcrPurpose::Auto | OcrPurpose::AutoTicks) {
            let scan = self.imgs.corrected_gray.as_ref().and_then(|g| find_power_labels(g, &panel.inner, &new_items));
            if let Some(scan) = scan {
                let task = OcrTask::new(Pt::new(0.0, 0.0), scan.canvas.clone(), None, Some(scan.boxes.clone()));
                self.text_ui.pending_powers = Some((new_items, scan));
                self.start_ocr(ctx, job.panel_id, OcrPurpose::AutoPowers, vec![task]);
                return;
            }
        }
        let w = self.project.work_mut(job.panel_id);
        match purpose {
            OcrPurpose::Auto | OcrPurpose::AutoTicks | OcrPurpose::AutoPowers => {
                let n_ticks = new_items.iter().filter(|i| matches!(i.role, TextRole::XTick | TextRole::YTick)).count();
                let note =
                    format!("Read {} text items ({} tick labels) in {:.1}s. Check the roles and values.", new_items.len(), n_ticks, dt);
                w.text.propose(new_items, note.clone());
                self.status = note;
            }
            OcrPurpose::Add => {
                let n = new_items.len();
                let items = w.text.value.get_or_insert_with(Vec::new);
                items.extend(new_items);
                self.status = format!("Added {n} text item(s).");
                self.edited(Stage::Text);
            }
            OcrPurpose::Replace(i) => {
                let text: String = lines.iter().map(|(l, _)| l.text.trim().to_string()).collect::<Vec<_>>().join(" ");
                if let Some(it) = w.text.value.as_mut().and_then(|v| v.get_mut(i)) {
                    it.text = plot2csv_ocr::labels::clean_label(&text);
                    it.value = label_value(&it.text);
                    it.conf = lines.iter().map(|(l, _)| l.conf).fold(0.0, f32::max);
                }
                self.status = format!("Re-read: “{text}”");
                self.edited(Stage::Text);
            }
        }
    }

    fn text_items(&self) -> Vec<TextItem> {
        self.project.active_work().and_then(|w| w.text.value.clone()).unwrap_or_default()
    }

    fn text_items_mut(&mut self) -> Option<&mut Vec<TextItem>> {
        let id = self.project.active_panel?;
        Some(self.project.work_mut(id).text.value.get_or_insert_with(Vec::new))
    }

    pub fn text_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        widgets::section(ui, "④ Text (OCR)");
        if let Some(w) = self.project.active_work() {
            if !w.text.note.is_empty() {
                ui.label(egui::RichText::new(&w.text.note).color(widgets::INFO));
            }
        }
        if self.ocr_job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading text…");
            });
        }
        ui.horizontal_wrapped(|ui| {
            let lbl = if self.text_ui.drawing {
                egui::Atoms::new("Drag a box on the image…")
            } else {
                egui::Atoms::new((icons::image(ui, Icon::Plus), "Read text in a box"))
            };
            if ui.selectable_label(self.text_ui.drawing, lbl).on_hover_text("Draw a box around text that was missed").clicked() {
                self.text_ui.drawing = !self.text_ui.drawing;
            }
            let sel = self.text_ui.selected;
            if ui.add_enabled(sel.is_some() && self.ocr_job.is_none(), egui::Button::new("Re-read selected")).clicked() {
                if let (Some(i), Some(id), Some(g)) = (sel, self.project.active_panel, self.imgs.corrected_gray.clone()) {
                    if let Some(it) = self.text_items().get(i) {
                        let r = it.bbox.expand(3.0);
                        let (off, img) = crop(&g, &r);
                        let full = [0.0f32, 0.0, img.width() as f32, img.height() as f32];
                        let task = if it.vertical {
                            let h = img.height() as f64;
                            let rot = image::imageops::rotate90(&img);
                            let b = vec![[0.0, 0.0, rot.width() as f32, rot.height() as f32]];
                            OcrTask::new(off, rot, Some(h), Some(b))
                        } else {
                            OcrTask::new(off, img, None, Some(vec![full]))
                        };
                        self.start_ocr(ctx, id, OcrPurpose::Replace(i), vec![task]);
                    }
                }
            }
        });
        ui.label(
            egui::RichText::new("Click a box to select it; double-click it to correct its text in place. Edit the text (white) or role below; the value (grey) is read from the text, and tick values feed the axis calibration.")
                .weak(),
        );
        ui.add_space(4.0);

        let inner = self.project.active_panel.and_then(|id| self.project.panel(id)).map(|p| p.inner);
        let mut items = self.text_items();
        let mut changed = false;
        let mut delete = None;
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| {
            (TextRole::ALL.iter().position(|r| *r == items[i].role), (items[i].bbox.y0 as i64) / 8, items[i].bbox.x0 as i64)
        });
        let scroll_to = std::mem::take(&mut self.text_ui.scroll_to_selected).then_some(self.text_ui.selected).flatten();
        let (text_fill, value_fill) = text_cell_colors(ui);
        egui::Grid::new("text_items").num_columns(4).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
            ui.label(egui::RichText::new("Role").strong());
            ui.label(egui::RichText::new("Text").strong());
            ui.label(egui::RichText::new("Value").strong());
            ui.label("");
            ui.end_row();
            for i in order {
                let it = &mut items[i];
                let sel = self.text_ui.selected == Some(i);
                ui.horizontal(|ui| {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 2.0, c32(it.role.color()));
                    if r.clicked() {
                        self.text_ui.selected = Some(i);
                    }
                    let role = egui::ComboBox::from_id_salt(("role", i)).width(70.0).selected_text(it.role.label()).show_ui(ui, |ui| {
                        for role in TextRole::ALL {
                            if ui.selectable_value(&mut it.role, role, role.label()).changed() {
                                it.manual = true;
                                changed = true;
                            }
                        }
                    });
                    role.response.on_hover_cursor(CursorIcon::PointingHand);
                });
                // Editable: a white field with a frame and the text cursor.
                let te = ui
                    .add(
                        egui::TextEdit::singleline(&mut it.text)
                            .desired_width(110.0)
                            .background_color(text_fill)
                            .text_color_opt(sel.then_some(widgets::INFO)),
                    )
                    .on_hover_cursor(CursorIcon::Text);
                if te.changed() {
                    it.value = label_value(&it.text);
                    it.manual = true;
                    changed = true;
                }
                if te.gained_focus() {
                    self.text_ui.selected = Some(i);
                }
                if scroll_to == Some(i) {
                    te.scroll_to_me(Some(egui::Align::Center));
                }
                // Read-only: a grey cell, no frame.
                let v = match it.value {
                    Some(v) => format!("{v}"),
                    None => "—".into(),
                };
                egui::Frame::new()
                    .fill(value_fill)
                    .corner_radius(2)
                    .inner_margin(egui::Margin::symmetric(4, 2))
                    .show(ui, |ui| {
                        ui.set_min_width(52.0);
                        ui.label(egui::RichText::new(v).monospace())
                    })
                    .response
                    .on_hover_text(format!("Read from the text (edit the text to change it). OCR confidence {:.0}%", it.conf * 100.0));
                if ui.small_button(icons::image(ui, Icon::TrashCan)).on_hover_text("Delete").clicked() {
                    delete = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = delete {
            items.remove(i);
            self.text_ui.selected = None;
            changed = true;
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Re-classify roles").on_hover_text("Re-assign roles of items you have not edited").clicked() {
                if let Some(inner) = inner {
                    classify(&mut items, &inner);
                    changed = true;
                }
            }
            if ui.button("Clear all").clicked() {
                items.clear();
                changed = true;
            }
        });
        if changed {
            if let Some(v) = self.text_items_mut() {
                *v = items;
            }
            self.edited(Stage::Text);
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("No text needed? Approve with an empty list and place the calibration points by hand in the next step.")
                .weak()
                .small(),
        );
    }

    pub fn text_canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, view: &CanvasView, interactive: bool) {
        let painter = ui.painter_at(view.rect);
        if let Some(p) = self.project.active_panel.and_then(|id| self.project.panel(id)) {
            let inner = view.rect_to_screen(to_rect(&p.inner));
            painter.rect_stroke(inner, 0.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 140, 255, 120)), StrokeKind::Middle);
            let outer = view.rect_to_screen(to_rect(&p.outer));
            painter.rect_stroke(outer, 0.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(110, 110, 110, 120)), StrokeKind::Middle);
        }
        let items = self.text_items();
        for (i, it) in items.iter().enumerate() {
            let r = view.rect_to_screen(to_rect(&it.bbox));
            let col = c32(it.role.color());
            let sel = self.text_ui.selected == Some(i);
            painter.rect_filled(r, 2.0, col.gamma_multiply(if sel { 0.35 } else { 0.12 }));
            painter.rect_stroke(r, 2.0, Stroke::new(if sel { 2.5 } else { 1.5 }, col), StrokeKind::Outside);
            if sel || view.zoom > 0.6 {
                let label = match it.value {
                    Some(v) if matches!(it.role, TextRole::XTick | TextRole::YTick) => format!("{v}"),
                    _ => it.role.label().to_string(),
                };
                widgets::label_box(&painter, r.left_top() - egui::vec2(0.0, 2.0), egui::Align2::LEFT_BOTTOM, &label, col);
            }
        }
        if let (Some(a), Some(b)) = (self.text_ui.draw_start, self.text_ui.draw_cur) {
            painter.rect_stroke(
                Rect::from_two_pos(view.to_screen(a), view.to_screen(b)),
                0.0,
                Stroke::new(2.0, widgets::INFO),
                StrokeKind::Middle,
            );
        }
        if !interactive {
            return;
        }
        let resp = &view.response;
        let pointer = resp.interact_pointer_pos();
        // The smallest box at a screen position (boxes can nest).
        let item_at = |p: Pos2| -> Option<usize> {
            let ip = super::pt(view.to_image(p));
            items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.bbox.expand(3.0).contains(ip))
                .min_by(|a, b| a.1.bbox.area().partial_cmp(&b.1.bbox.area()).unwrap())
                .map(|(i, _)| i)
        };
        if self.text_ui.drawing {
            view.set_cursor(CursorIcon::Crosshair);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.text_ui.draw_start = pointer.map(|p| view.to_image(p));
            }
            if resp.dragged_by(egui::PointerButton::Primary) {
                self.text_ui.draw_cur = pointer.map(|p| view.to_image(p));
            }
            if resp.drag_stopped() {
                if let (Some(a), Some(b), Some(id), Some(g)) = (
                    self.text_ui.draw_start.take(),
                    self.text_ui.draw_cur.take(),
                    self.project.active_panel,
                    self.imgs.corrected_gray.clone(),
                ) {
                    let r = RectF::new(a.x as f64, a.y as f64, b.x as f64, b.y as f64);
                    if r.width() > 4.0 && r.height() > 4.0 {
                        let (off, img) = crop(&g, &r);
                        // Tall narrow boxes are probably vertical text.
                        let task = if r.height() > 2.5 * r.width() {
                            let h = img.height() as f64;
                            OcrTask::new(off, image::imageops::rotate90(&img), Some(h), None)
                        } else {
                            OcrTask::new(off, img, None, None)
                        };
                        self.start_ocr(ctx, id, OcrPurpose::Add, vec![task]);
                        self.text_ui.drawing = false;
                    }
                }
            }
            return;
        }
        // A box: click selects, double-click edits its text in place.
        if resp.hover_pos().and_then(item_at).is_some() {
            view.set_cursor(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if let Some(p) = pointer {
                self.text_ui.selected = item_at(p);
                self.text_ui.scroll_to_selected = self.text_ui.selected.is_some();
            }
        }
        if resp.double_clicked() {
            if let Some(i) = pointer.and_then(item_at) {
                self.text_ui.selected = Some(i);
                self.text_ui.scroll_to_selected = true;
                self.text_ui.inline = Some(InlineEdit { index: i, text: items[i].text.clone(), focused: false });
            }
        }
        self.inline_text_editor(ctx, view, &items);
        if let (Some(i), true) = (
            self.text_ui.selected,
            ctx.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace))
                && !ctx.egui_wants_keyboard_input(),
        ) {
            if let Some(v) = self.text_items_mut() {
                if i < v.len() {
                    v.remove(i);
                }
            }
            self.text_ui.selected = None;
            self.edited(Stage::Text);
        }
    }

    /// The in-place editor over a text box (opened by double-clicking it):
    /// Enter or clicking elsewhere keeps the text, Esc cancels.
    fn inline_text_editor(&mut self, ctx: &egui::Context, view: &CanvasView, items: &[TextItem]) {
        let Some(ed) = self.text_ui.inline.as_mut() else { return };
        let Some(it) = items.get(ed.index) else {
            self.text_ui.inline = None;
            return;
        };
        let r = view.rect_to_screen(to_rect(&it.bbox));
        let col = c32(it.role.color());
        let size = (0.8 * r.height()).clamp(13.0, 28.0);
        let width = (r.width() + 24.0).max(90.0);
        let id = egui::Id::unique("text_inline_edit");
        let area = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(r.left_top() - egui::vec2(3.0, 3.0)).show(
            ctx,
            |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(2)
                    .stroke(Stroke::new(2.0, col))
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.text)
                                .id(id)
                                .font(egui::FontId::proportional(size))
                                .desired_width(width)
                                .background_color(text_cell_colors(ui).0),
                        )
                    })
                    .inner
            },
        );
        let te = area.inner;
        if !ed.focused {
            te.request_focus();
            ed.focused = true;
            return;
        }
        if !te.lost_focus() && te.has_focus() {
            return;
        }
        let cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let Some(ed) = self.text_ui.inline.take() else { return };
        if cancel || ed.text == it.text {
            return;
        }
        if let Some(v) = self.text_items_mut().and_then(|v| v.get_mut(ed.index)) {
            v.text = ed.text.trim().to_string();
            v.value = label_value(&v.text);
            v.manual = true;
        }
        self.status = format!("Text set to “{}”.", ed.text.trim());
        self.edited(Stage::Text);
    }
}
