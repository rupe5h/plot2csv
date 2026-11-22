//! Top-level application: layout, stage flow, jobs and menus.

use std::sync::mpsc::Receiver;

use egui::{Align2, Color32, Pos2, Vec2};
use plot2csv_core::geom::Pt;
use plot2csv_core::history::{Entry, History};
use plot2csv_core::project::{ImportOptions, Project, SourceImage, Stage, StageStatus};
use plot2csv_core::table::TableOptions;
use plot2csv_ocr::TextLine;

use crate::canvas::{CanvasView, ImageCanvas};
use crate::dialogs::Dialog;
use crate::icons::{self, Icon};
use crate::images::Images;
use crate::measure::MeasureUi;
use crate::platform;
use crate::stages::{AxesUi, CorrectUi, DataUi, ExportUi, RegionUi, TextUi};
use crate::widgets;

type Task = Box<dyn FnOnce(&mut Plot2CsvApp, &egui::Context)>;

/// Work to run after a "Working…" overlay has been painted.
struct Busy {
    msg: String,
    frames: u8,
    task: Option<Task>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HistoryOp {
    Undo,
    Redo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilePurpose {
    Image,
    Project,
}

pub struct OcrJob {
    pub panel_id: u32,
    pub purpose: crate::stages::OcrPurpose,
    /// The crop being read now.
    pub current: crate::stages::OcrTask,
    pub rx: Receiver<Result<Vec<TextLine>, String>>,
    /// More crops to read after this one.
    pub queue: Vec<crate::stages::OcrTask>,
    /// Lines found so far (corrected-image px) and whether they were vertical.
    pub collected: Vec<(TextLine, bool)>,
}

pub struct Plot2CsvApp {
    pub project: Project,
    pub stage: Stage,
    pub imgs: Images,
    pub canvas: ImageCanvas,
    pub mag_zoom: f32,
    /// Image position for the magnifier (hover, or a point being nudged).
    pub mag_center: Option<Pos2>,
    pub file_job: Option<(FilePurpose, Receiver<platform::PickedFile>)>,
    pub ocr_job: Option<OcrJob>,
    pub dialog: Option<Dialog>,
    pub status: String,
    busy: std::collections::VecDeque<Busy>,
    pub dark: bool,
    pub correct_ui: CorrectUi,
    pub region_ui: RegionUi,
    pub text_ui: TextUi,
    pub axes_ui: AxesUi,
    pub data_ui: DataUi,
    pub export_ui: ExportUi,
    pub measure: MeasureUi,
    pub table_opts: TableOptions,
    pub show_overlays: bool,
    pub tips: crate::tips::Tips,
    /// Undo/redo snapshots of the project.
    pub history: History,
    /// Undo/redo requested this frame, applied at the start of the next one.
    history_op: Option<HistoryOp>,
}

impl Plot2CsvApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        widgets::apply_theme(&cc.egui_ctx, false);
        Self {
            project: Project::default(),
            stage: Stage::Import,
            imgs: Images::default(),
            canvas: ImageCanvas::default(),
            mag_zoom: 5.0,
            mag_center: None,
            file_job: None,
            ocr_job: None,
            dialog: None,
            status: "Load a plot image: File → Load Image, drag-and-drop, or paste with Ctrl+V.".into(),
            busy: std::collections::VecDeque::new(),
            dark: false,
            correct_ui: CorrectUi::default(),
            region_ui: RegionUi::default(),
            text_ui: TextUi::default(),
            axes_ui: AxesUi::default(),
            data_ui: DataUi::default(),
            export_ui: ExportUi::default(),
            measure: MeasureUi::default(),
            table_opts: TableOptions { format: plot2csv_core::NumFormat::Fixed, digits: 4, ..Default::default() },
            show_overlays: true,
            tips: crate::tips::Tips::load(),
            history: History::default(),
            history_op: None,
        }
    }

    // ------------------------------------------------------------------ jobs

    /// Show a busy overlay, then run `task` on a later frame.
    pub fn defer(&mut self, msg: impl Into<String>, task: impl FnOnce(&mut Plot2CsvApp, &egui::Context) + 'static) {
        self.busy.push_back(Busy { msg: msg.into(), frames: 0, task: Some(Box::new(task)) });
    }

    pub fn is_busy(&self) -> bool {
        !self.busy.is_empty() || self.ocr_job.is_some()
    }

    fn run_busy(&mut self, ctx: &egui::Context) {
        if let Some(b) = self.busy.front_mut() {
            if b.frames < 2 {
                b.frames += 1;
                ctx.request_repaint();
                return;
            }
            let task = b.task.take();
            self.busy.pop_front();
            if let Some(t) = task {
                t(self, ctx);
            }
            ctx.request_repaint();
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some((purpose, rx)) = &self.file_job {
            match rx.try_recv() {
                Ok(f) => {
                    let purpose = *purpose;
                    self.file_job = None;
                    match purpose {
                        FilePurpose::Image => self.load_image_bytes(ctx, f.name, f.bytes),
                        FilePurpose::Project => self.load_project_bytes(ctx, &f.bytes),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.file_job = None,
                Err(_) => {}
            }
        }
        if let Some(bytes) = platform::take_pasted_image() {
            self.load_image_bytes(ctx, "pasted image".into(), bytes);
        }
        let dropped: Vec<egui::DroppedFileHandle> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.into_iter().next() {
            let is_json = f.path().extension().map(|e| e == "json").unwrap_or(false);
            let purpose = if is_json { FilePurpose::Project } else { FilePurpose::Image };
            self.file_job = Some((purpose, platform::read_dropped(ctx, f)));
        }
        // Native clipboard images arrive as egui events.
        let pasted_img: Option<Vec<u8>> = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Paste(s) if s.starts_with("data:image/") => {
                    use base64::Engine;
                    s.split_once(',').and_then(|(_, b)| base64::engine::general_purpose::STANDARD.decode(b).ok())
                }
                _ => None,
            })
        });
        if let Some(b) = pasted_img {
            self.load_image_bytes(ctx, "pasted image".into(), b);
        }
        self.poll_ocr(ctx);
    }

    // ------------------------------------------------------------- loading

    pub fn open_image_dialog(&mut self, ctx: &egui::Context) {
        self.file_job = Some((FilePurpose::Image, platform::pick_file(ctx, &[("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])])));
    }

    pub fn open_project_dialog(&mut self, ctx: &egui::Context) {
        self.file_job = Some((FilePurpose::Project, platform::pick_file(ctx, &[("plot2csv project", &["json"])])));
    }

    pub fn load_image_bytes(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>) {
        if bytes.first() == Some(&b'{') {
            self.load_project_bytes(ctx, &bytes);
            return;
        }
        let img = match crate::images::decode(&bytes) {
            Ok(i) => i,
            Err(e) => {
                self.status = format!("Could not read {name}: {e}");
                return;
            }
        };
        let mut p = Project::default();
        p.source = Some(SourceImage { name: name.clone(), width: img.width(), height: img.height(), bytes });
        p.import.propose(ImportOptions::default(), "");
        self.project = p;
        self.reset_ui_state();
        self.imgs.clear();
        self.stage = Stage::Import;
        self.history.reset(&self.project, self.stage);
        self.status = format!("Loaded {name} ({}×{}). Review it and press “Approve & continue”.", img.width(), img.height());
        self.sync_images(ctx);
    }

    pub fn load_project_bytes(&mut self, ctx: &egui::Context, bytes: &[u8]) {
        match std::str::from_utf8(bytes).map_err(|e| e.to_string()).and_then(Project::from_json) {
            Ok(p) => {
                self.project = p;
                self.reset_ui_state();
                self.imgs.clear();
                self.sync_images(ctx);
                // Open the furthest stage that has content.
                self.stage = Stage::ALL
                    .iter()
                    .rev()
                    .copied()
                    .find(|s| self.project.status(*s) != StageStatus::Pending && self.project.can_open(*s))
                    .unwrap_or(Stage::Import);
                self.history.reset(&self.project, self.stage);
                self.status = "Project loaded.".into();
            }
            Err(e) => self.status = e,
        }
    }

    fn reset_ui_state(&mut self) {
        self.correct_ui = CorrectUi::default();
        self.region_ui = RegionUi::default();
        self.text_ui = TextUi::default();
        self.axes_ui = AxesUi::default();
        self.data_ui = DataUi::default();
        self.export_ui = ExportUi::default();
        self.measure = MeasureUi::default();
        self.dialog = None;
        self.canvas.fit();
    }

    pub fn sync_images(&mut self, ctx: &egui::Context) {
        match self.imgs.sync_source(ctx, &self.project) {
            Ok(true) => {
                self.canvas.fit();
            }
            Ok(false) => {}
            Err(e) => self.status = format!("Image error: {e}"),
        }
        if self.stage > Stage::Correct && !self.imgs.corrected_is_current(&self.project) {
            self.imgs.sync_corrected(ctx, &self.project);
            self.data_ui.invalidate_caches();
        }
    }

    pub fn save_project(&mut self) {
        let name = self
            .project
            .source
            .as_ref()
            .map(|s| s.name.rsplit_once('.').map(|(a, _)| a).unwrap_or(&s.name).to_string())
            .unwrap_or_else(|| "plot2csv".into());
        platform::save_file(&format!("{name}.plot2csv.json"), self.project.to_json().into_bytes());
        self.status = "Project saved.".into();
    }

    // ----------------------------------------------------------- stage flow

    /// The image shown on the canvas for the current stage (source before
    /// correction is approved, corrected afterwards).
    pub fn shows_source(&self) -> bool {
        matches!(self.stage, Stage::Import | Stage::Correct) && !self.correct_ui.preview
    }

    pub fn goto(&mut self, ctx: &egui::Context, stage: Stage) {
        if !self.project.can_open(stage) {
            return;
        }
        self.stage = stage;
        self.measure.active = None;
        if stage > Stage::Correct && !self.imgs.corrected_is_current(&self.project) {
            self.defer("Applying correction…", |app, ctx| {
                app.imgs.sync_corrected(ctx, &app.project);
                app.data_ui.invalidate_caches();
                app.canvas.fit();
            });
        }
        self.canvas.fit();
        let _ = ctx;
    }

    /// Mark the current stage as edited by the user: it needs re-approval and
    /// later stages become stale.
    pub fn edited(&mut self, stage: Stage) {
        let p = &mut self.project;
        let was_approved = p.status(stage) == StageStatus::Approved;
        match stage {
            Stage::Import => {
                if p.import.status == StageStatus::Approved {
                    p.import.status = StageStatus::Proposed;
                }
            }
            Stage::Correct => {
                if p.correction.status == StageStatus::Approved {
                    p.correction.status = StageStatus::Proposed;
                }
                // Fine-tune corners were placed on the old corrected image.
                p.fine_tune = None;
            }
            Stage::Region => {
                if p.regions.status == StageStatus::Approved {
                    p.regions.status = StageStatus::Proposed;
                }
            }
            Stage::Text | Stage::Axes | Stage::Data => {
                if let Some(id) = p.active_panel {
                    let w = p.work_mut(id);
                    let st = match stage {
                        Stage::Text => &mut w.text.status,
                        Stage::Axes => &mut w.axes.status,
                        _ => &mut w.data.status,
                    };
                    if *st == StageStatus::Approved || *st == StageStatus::Stale {
                        *st = StageStatus::Proposed;
                    }
                }
            }
            Stage::Export => {}
        }
        if was_approved {
            p.invalidate_after(stage);
        }
    }

    /// Run the automatic proposal for `stage` (the next stage after an approval).
    pub fn run_auto(&mut self, ctx: &egui::Context, stage: Stage) {
        match stage {
            Stage::Import => {}
            Stage::Correct => self.defer("Detecting rotation and perspective…", |app, ctx| app.auto_correct(ctx)),
            Stage::Region => self.defer("Finding the plot area…", |app, ctx| {
                app.imgs.sync_corrected(ctx, &app.project);
                app.data_ui.invalidate_caches();
                app.auto_regions(ctx)
            }),
            Stage::Text => self.auto_text(ctx),
            Stage::Axes => self.defer("Inferring axis scale…", |app, ctx| app.auto_axes(ctx)),
            Stage::Data => self.defer("Detecting data series…", |app, ctx| app.auto_data(ctx)),
            Stage::Export => {}
        }
    }

    /// Can the current stage be approved, and if not, why?
    pub fn approve_blocker(&self) -> Option<String> {
        let p = &self.project;
        match self.stage {
            Stage::Import => p.source.is_none().then(|| "Load an image first.".into()),
            Stage::Correct => p.correction.value.is_none().then(|| "Waiting for the correction proposal.".into()),
            Stage::Region => {
                let n = p.panel_list().iter().filter(|q| q.enabled).count();
                if p.fine_tune.is_some() {
                    Some("Apply or cancel the perspective fine-tune first.".into())
                } else {
                    (n == 0).then(|| "Add at least one plot area.".into())
                }
            }
            Stage::Text => p.active_work().and_then(|w| w.text.value.as_ref()).is_none().then(|| "Waiting for OCR.".into()),
            Stage::Axes => match p.active_work().and_then(|w| w.axes.value.as_ref()) {
                None => Some("No calibration yet.".into()),
                Some(a) => a.calibrate().err(),
            },
            Stage::Data => p.active_work().and_then(|w| w.data.value.as_ref()).is_none().then(|| "No datasets yet.".into()),
            Stage::Export => Some("This is the last stage.".into()),
        }
    }

    pub fn approve_current(&mut self, ctx: &egui::Context) {
        if self.approve_blocker().is_some() {
            return;
        }
        let stage = self.stage;
        let p = &mut self.project;
        match stage {
            Stage::Import => p.import.approve(),
            Stage::Correct => p.correction.approve(),
            Stage::Region => {
                p.regions.approve();
                p.sync_panels();
            }
            Stage::Text | Stage::Axes | Stage::Data => {
                if let Some(id) = p.active_panel {
                    let w = p.work_mut(id);
                    match stage {
                        Stage::Text => w.text.approve(),
                        Stage::Axes => w.axes.approve(),
                        _ => w.data.approve(),
                    }
                }
            }
            Stage::Export => {}
        }
        let Some(next) = stage.next() else { return };
        let next_status = self.project.status(next);
        if matches!(next_status, StageStatus::Pending | StageStatus::Stale) && next != Stage::Export {
            self.run_auto(ctx, next);
        }
        self.goto(ctx, next);
        self.status = format!("{} approved. Review the proposal for “{}”.", stage.title(), next.title());
    }

    // -------------------------------------------------------------- history

    /// Nothing in flux: no job running, no drag, no text field being edited.
    fn settled(&self, ctx: &egui::Context) -> bool {
        !self.is_busy() && !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input()
    }

    /// Record this frame's project state as an undo step if it changed.
    fn track_history(&mut self, ctx: &egui::Context) {
        if !self.settled(ctx) {
            return;
        }
        let nudge_at = crate::stages::arrow_pressed(ctx).then(|| ctx.input(|i| i.time));
        self.history.track(&self.project, self.stage, nudge_at);
    }

    /// Undo or redo at the start of the next frame.
    pub fn request_history(&mut self, ctx: &egui::Context, op: HistoryOp) {
        self.history_op = Some(op);
        ctx.request_repaint();
    }

    /// Spelled-out shortcut (the bundled fonts have no ⌘ glyph).
    pub fn shortcut_name(ctx: &egui::Context, op: HistoryOp) -> &'static str {
        let mac = ctx.os() == egui::os::OperatingSystem::Mac;
        match (op, mac) {
            (HistoryOp::Undo, true) => "Cmd+Z",
            (HistoryOp::Undo, false) => "Ctrl+Z",
            (HistoryOp::Redo, true) => "Cmd+Shift+Z",
            (HistoryOp::Redo, false) => "Ctrl+Y",
        }
    }

    fn apply_history_op(&mut self, ctx: &egui::Context) {
        let Some(op) = self.history_op.take() else { return };
        if !self.settled(ctx) {
            return;
        }
        let entry = match op {
            HistoryOp::Undo => self.history.undo(&self.project, self.stage),
            HistoryOp::Redo => self.history.redo(&self.project, self.stage),
        };
        match entry {
            Some(e) => {
                self.restore(ctx, e);
                self.status = match op {
                    HistoryOp::Undo => format!("Undone. Redo with {}.", Self::shortcut_name(ctx, HistoryOp::Redo)),
                    HistoryOp::Redo => "Redone.".into(),
                };
            }
            None => {
                self.status = match op {
                    HistoryOp::Undo => "Nothing to undo.".into(),
                    HistoryOp::Redo => "Nothing to redo.".into(),
                }
            }
        }
    }

    /// Put a history snapshot back (keeping the loaded source image) and show
    /// the stage it was made in.
    fn restore(&mut self, ctx: &egui::Context, e: Entry) {
        let mut p = e.project;
        p.source = self.project.source.take();
        self.project = p;
        let mut stage = e.stage;
        while !self.project.can_open(stage) {
            stage = stage.prev().unwrap_or(Stage::Import);
        }
        if stage != self.stage {
            self.goto(ctx, stage);
        }
        self.correct_ui.clear_transient();
        self.region_ui.clear_transient();
        self.text_ui.clear_transient();
        self.axes_ui.clear_transient();
        self.data_ui.clear_transient();
        self.measure.clear_transient();
        self.resume_placing();
    }

    // ------------------------------------------------------------------ UI

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Load Image…").clicked() {
                    self.open_image_dialog(ctx);
                }
                ui.label(egui::RichText::new("…or drag-and-drop / Ctrl+V paste").weak().small());
                ui.separator();
                if ui.button("Import Project (JSON)…").clicked() {
                    self.open_project_dialog(ctx);
                }
                if ui.add_enabled(self.project.source.is_some(), egui::Button::new("Export Project (JSON)")).clicked() {
                    self.save_project();
                }
                ui.separator();
                if ui.add_enabled(self.imgs.corrected.is_some(), egui::Button::new("Save Corrected Image (PNG)")).clicked() {
                    if let Some(img) = &self.imgs.corrected {
                        let mut buf = std::io::Cursor::new(Vec::new());
                        if img.write_to(&mut buf, image::ImageFormat::Png).is_ok() {
                            platform::save_file("corrected.png", buf.into_inner());
                        }
                    }
                }
                if ui.button("New (clear)").clicked() {
                    self.project = Project::default();
                    self.imgs.clear();
                    self.reset_ui_state();
                    self.stage = Stage::Import;
                    self.history.reset(&self.project, self.stage);
                }
            });
            ui.menu_button("Edit", |ui| {
                let idle = !self.is_busy();
                let undo = egui::Button::new("Undo").shortcut_text(Self::shortcut_name(ctx, HistoryOp::Undo));
                if ui.add_enabled(idle && self.history.can_undo(), undo).clicked() {
                    self.request_history(ctx, HistoryOp::Undo);
                }
                let redo = egui::Button::new("Redo").shortcut_text(Self::shortcut_name(ctx, HistoryOp::Redo));
                if ui.add_enabled(idle && self.history.can_redo(), redo).clicked() {
                    self.request_history(ctx, HistoryOp::Redo);
                }
            });
            ui.menu_button("Axes", |ui| {
                let can = self.project.can_open(Stage::Axes);
                if ui.add_enabled(can, egui::Button::new("Calibrate Axes")).clicked() {
                    self.goto(ctx, Stage::Axes);
                }
                if ui.add_enabled(self.project.active_panel.is_some(), egui::Button::new("Choose Plot Type…")).clicked() {
                    self.dialog = Some(Dialog::PlotType);
                }
                if ui.add_enabled(can, egui::Button::new("Transformation Equations")).clicked() {
                    self.dialog = Some(Dialog::Equations);
                }
            });
            ui.menu_button("Data", |ui| {
                let can = self.project.can_open(Stage::Data);
                if ui.add_enabled(can, egui::Button::new("Manage Datasets…")).clicked() {
                    self.dialog = Some(Dialog::Datasets);
                }
                if ui.add_enabled(can, egui::Button::new("View Data…")).clicked() {
                    self.dialog = Some(Dialog::AcquiredData);
                }
                if ui.add_enabled(can, egui::Button::new("Export all (ZIP)")).clicked() {
                    self.export_zip();
                }
            });
            ui.menu_button("Analyze", |ui| {
                let can = self.imgs.corrected.is_some() && self.project.active_panel.is_some();
                if ui.add_enabled(can, egui::Button::new("Measure Distances")).clicked() {
                    self.measure.start(crate::measure::MeasureMode::Distance);
                }
                if ui.add_enabled(can, egui::Button::new("Measure Angles")).clicked() {
                    self.measure.start(crate::measure::MeasureMode::Angle);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.checkbox(&mut self.dark, "Dark mode").changed() {
                    widgets::apply_theme(ctx, self.dark);
                }
                ui.checkbox(&mut self.show_overlays, "Show overlays");
                ui.horizontal(|ui| {
                    ui.label("Magnifier zoom");
                    ui.add(egui::Slider::new(&mut self.mag_zoom, 2.0..=16.0).step_by(1.0));
                });
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Tip for this stage").clicked() {
                    self.tips.reopen(self.stage);
                }
                if ui.button("Keyboard Shortcuts").clicked() {
                    self.dialog = Some(Dialog::Shortcuts);
                }
                ui.separator();
                if crate::platform::offline_copy_available()
                    && ui
                        .button("Download for Offline Use")
                        .on_hover_text("Save this app as one HTML file. Open it from disk later; it works without internet.")
                        .clicked()
                {
                    self.status = match crate::platform::save_offline_copy("plot2csv.html") {
                        Ok(bytes) => format!(
                            "Downloading plot2csv.html ({:.0} MB). Open it from your downloads folder any time; it works offline.",
                            bytes as f64 / 1e6
                        ),
                        Err(e) => format!("Could not save an offline copy: {e}"),
                    };
                }
                if ui.button("Licences").clicked() {
                    self.dialog = Some(Dialog::Licenses);
                }
                if ui.button("About").clicked() {
                    self.dialog = Some(Dialog::About);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    self.canvas.fit();
                }
                let mut z = self.canvas.zoom * 100.0;
                let r = ui.add(egui::DragValue::new(&mut z).speed(1.0).range(2.0..=6400.0).suffix("%"));
                if r.changed() {
                    self.canvas.set_zoom(z / 100.0);
                }
                if ui.button("−").on_hover_text("Zoom out (−)").clicked() {
                    self.canvas.zoom_by(1.0 / 1.25, None);
                }
                if ui.button("+").on_hover_text("Zoom in (+)").clicked() {
                    self.canvas.zoom_by(1.25, None);
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Magnifier + readout (WPD's zoomed view)
        let (tex, size) = self.display_texture();
        let side = ui.available_width().min(260.0);
        ui.vertical_centered(|ui| {
            let r = widgets::magnifier(ui, tex.as_ref(), size, self.mag_center, self.mag_zoom, side);
            let gear = egui::Rect::from_min_size(r.right_top() + Vec2::new(-22.0, 2.0), Vec2::splat(20.0));
            let gr = ui
                .interact(gear, ui.scope_id().with("mag_gear"), egui::Sense::click())
                .on_hover_text(format!("Magnifier zoom {}× (click to change)", self.mag_zoom));
            ui.painter().rect_filled(gear, 3.0, Color32::from_white_alpha(200));
            icons::paint(ui, Icon::Gear, gear.center(), 14.0, Color32::from_gray(60));
            if gr.clicked() {
                self.mag_zoom = if self.mag_zoom >= 16.0 { 2.0 } else { self.mag_zoom * 2.0 };
            }
            ui.label(egui::RichText::new(self.readout()).monospace());
        });
        ui.separator();
        if self.measure.active.is_some() {
            self.measure_sidebar(ui, ctx);
            ui.separator();
        }
        // Stale banner
        let st = self.project.status(self.stage);
        if st == StageStatus::Stale {
            egui::Frame::new().fill(Color32::from_rgb(255, 236, 200)).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
                ui.label(egui::RichText::new("An earlier stage changed since this was produced.").color(Color32::BLACK));
                ui.horizontal(|ui| {
                    if ui.button("Re-run auto").clicked() {
                        self.run_auto(ctx, self.stage);
                    }
                    if ui.button("Keep my edits").clicked() {
                        self.keep_stale_edits();
                    }
                });
            });
        }
        let footer_h = 96.0;
        let avail = ui.available_height() - footer_h;
        egui::ScrollArea::vertical().max_height(avail.max(100.0)).auto_shrink([false, false]).show(ui, |ui| match self.stage {
            Stage::Import => self.import_sidebar(ui, ctx),
            Stage::Correct => self.correct_sidebar(ui, ctx),
            Stage::Region => self.region_sidebar(ui, ctx),
            Stage::Text => self.text_sidebar(ui, ctx),
            Stage::Axes => self.axes_sidebar(ui, ctx),
            Stage::Data => self.data_sidebar(ui, ctx),
            Stage::Export => self.export_sidebar(ui, ctx),
        });
        // Footer: approve / re-run / reset
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            if self.stage != Stage::Export {
                let blocker = self.approve_blocker();
                let approved = self.project.status(self.stage) == StageStatus::Approved;
                let label = if approved { "Continue" } else { "Approve & continue" };
                let r = widgets::primary_button_trailing(ui, label, Icon::ChevronRight, blocker.is_none() && !self.is_busy());
                if let Some(b) = &blocker {
                    r.clone().on_disabled_hover_text(b.clone());
                }
                if r.clicked() {
                    self.approve_current(ctx);
                }
                ui.horizontal(|ui| {
                    let has_auto = !matches!(self.stage, Stage::Import);
                    if has_auto
                        && ui
                            .add_enabled(!self.is_busy(), egui::Button::new((icons::image(ui, Icon::ArrowsRotate), "Re-run auto")))
                            .on_hover_text("Discard edits and run the automatic step again")
                            .clicked()
                    {
                        self.run_auto(ctx, self.stage);
                    }
                    if has_auto
                        && ui
                            .button((icons::image(ui, Icon::RotateLeft), "Reset to auto"))
                            .on_hover_text("Restore the last automatic proposal")
                            .clicked()
                    {
                        self.reset_stage_to_auto();
                    }
                });
                if let Some(b) = blocker {
                    ui.label(egui::RichText::new(b).small().color(widgets::WARN));
                }
            }
        });
    }

    fn keep_stale_edits(&mut self) {
        let p = &mut self.project;
        match self.stage {
            Stage::Correct => p.correction.status = StageStatus::Proposed,
            Stage::Region => p.regions.status = StageStatus::Proposed,
            Stage::Text | Stage::Axes | Stage::Data => {
                if let Some(id) = p.active_panel {
                    let w = p.work_mut(id);
                    match self.stage {
                        Stage::Text => w.text.status = StageStatus::Proposed,
                        Stage::Axes => w.axes.status = StageStatus::Proposed,
                        _ => w.data.status = StageStatus::Proposed,
                    }
                }
            }
            _ => {}
        }
    }

    fn reset_stage_to_auto(&mut self) {
        let p = &mut self.project;
        match self.stage {
            Stage::Correct => {
                p.correction.reset_to_auto();
                p.fine_tune = None;
            }
            Stage::Region => {
                p.regions.reset_to_auto();
                p.sync_panels();
                p.fine_tune = None;
            }
            Stage::Text | Stage::Axes | Stage::Data => {
                if let Some(id) = p.active_panel {
                    let w = p.work_mut(id);
                    match self.stage {
                        Stage::Text => w.text.reset_to_auto(),
                        Stage::Axes => w.axes.reset_to_auto(),
                        _ => w.data.reset_to_auto(),
                    }
                }
            }
            _ => {}
        }
        self.data_ui.invalidate_caches();
    }

    pub fn display_texture(&self) -> (Option<egui::TextureHandle>, Vec2) {
        if self.shows_source() {
            (self.imgs.source_tex.clone(), self.imgs.source_size())
        } else if self.imgs.corrected.is_some() {
            (self.imgs.corrected_tex.clone(), self.imgs.corrected_size())
        } else {
            (self.imgs.source_tex.clone(), self.imgs.source_size())
        }
    }

    /// Coordinates under the cursor: data values once calibrated, else pixels.
    fn readout(&self) -> String {
        let Some(p) = self.mag_center else { return "[ — , — ]".into() };
        if !self.shows_source() && self.stage >= Stage::Axes {
            if let Some(cal) = self.active_calibration() {
                let d = cal.to_data(Pt::new(p.x as f64, p.y as f64));
                let parts: Vec<String> = d
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let col = if matches!(cal, plot2csv_core::Calibration::Bar(_)) { 1 } else { i };
                        cal.format_value(col, *v, plot2csv_core::NumFormat::Exponential, 4)
                    })
                    .collect();
                return format!("[{}]", parts.join(", "));
            }
        }
        format!("[{:.2}, {:.2}] px", p.x, p.y)
    }

    pub fn active_calibration(&self) -> Option<plot2csv_core::Calibration> {
        self.project.active_work()?.axes.value.as_ref()?.calibrate().ok()
    }

    fn canvas_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (tex, size) = self.display_texture();
        let pan_with_primary = self.measure.active.is_none() && self.stage_pans_with_primary();
        let view = self.canvas.show(ui, tex.as_ref(), size, pan_with_primary);
        self.mag_center = view.hover.or(self.mag_center_override());
        let painter = ui.painter_at(view.rect);
        if self.project.source.is_none() {
            painter.text(
                view.rect.center(),
                Align2::CENTER_CENTER,
                "Drop a plot image here\nor use File → Load Image / Ctrl+V",
                egui::FontId::proportional(22.0),
                Color32::GRAY,
            );
            if view.response.double_clicked() {
                self.open_image_dialog(ctx);
            }
            return;
        }
        if self.measure.active.is_some() && !self.shows_source() {
            self.measure_canvas(ui, &view);
            self.stage_canvas(ui, ctx, &view, false);
        } else {
            self.stage_canvas(ui, ctx, &view, true);
        }
    }

    fn stage_pans_with_primary(&self) -> bool {
        match self.stage {
            Stage::Import | Stage::Export => true,
            Stage::Correct => !self.correct_ui.editing_quad(),
            Stage::Text => !self.text_ui.drawing,
            Stage::Data => self.data_ui.pans_with_primary(),
            _ => false,
        }
    }

    fn mag_center_override(&self) -> Option<Pos2> {
        match self.stage {
            Stage::Axes => self.axes_ui.focus,
            Stage::Data => self.data_ui.focus,
            Stage::Correct => self.correct_ui.focus,
            Stage::Region => self.region_ui.focus,
            _ => None,
        }
    }

    fn stage_canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, view: &CanvasView, interactive: bool) {
        match self.stage {
            Stage::Import => {}
            Stage::Correct => self.correct_canvas(ui, view, interactive),
            Stage::Region => self.region_canvas(ui, view, interactive),
            Stage::Text => self.text_canvas(ui, ctx, view, interactive),
            Stage::Axes => self.axes_canvas(ui, ctx, view, interactive),
            Stage::Data => self.data_canvas(ui, ctx, view, interactive),
            Stage::Export => self.export_canvas(ui, view),
        }
    }

    /// `typing`: a text field had the keyboard when the frame started. Its
    /// Enter (which also ends the editing) must not approve the stage.
    fn global_keys(&mut self, ctx: &egui::Context, typing: bool) {
        if typing || ctx.egui_wants_keyboard_input() || self.dialog.is_some() {
            return;
        }
        // Redo first: Cmd+Z also matches Cmd+Shift+Z.
        let (redo, undo) = ctx.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as Shortcut, Modifiers};
            let redo = i.consume_shortcut(&Shortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                || i.consume_shortcut(&Shortcut::new(Modifiers::COMMAND, Key::Y));
            let undo = i.consume_shortcut(&Shortcut::new(Modifiers::COMMAND, Key::Z));
            (redo, undo)
        });
        if !self.is_busy() {
            if redo {
                self.request_history(ctx, HistoryOp::Redo);
            } else if undo {
                self.request_history(ctx, HistoryOp::Undo);
            }
        }
        let (plus, minus, fit, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if plus {
            self.canvas.zoom_by(1.25, None);
        }
        if minus {
            self.canvas.zoom_by(1.0 / 1.25, None);
        }
        if fit {
            self.canvas.fit();
        }
        if enter && self.approve_blocker().is_none() && self.stage != Stage::Export && !self.is_busy() {
            self.approve_current(ctx);
        }
    }

    fn busy_overlay(&self, ctx: &egui::Context) {
        let msg = match (self.busy.front(), &self.ocr_job) {
            (Some(b), _) => b.msg.clone(),
            (None, Some(_)) => "Reading text (OCR)…".to_string(),
            _ => return,
        };
        egui::Area::new(egui::Id::unique("busy")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(
            ctx,
            |ui| {
                egui::Frame::popup(ui.style()).inner_margin(16.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(msg).size(18.0));
                    });
                });
            },
        );
    }
}

impl eframe::App for Plot2CsvApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let typing = ctx.egui_wants_keyboard_input();
        self.run_busy(&ctx);
        self.poll_jobs(&ctx);
        self.apply_history_op(&ctx);
        if self.busy.is_empty() {
            self.sync_images(&ctx);
        }
        // Low-frequency poll so pasted images and async results show up.
        ctx.request_repaint_after(std::time::Duration::from_millis(300));

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &ctx));
        egui::Panel::top("stepper").show(ui, |ui| {
            ui.add_space(2.0);
            let current = self.stage;
            let clicked = {
                let p = &self.project;
                widgets::stepper(ui, current, |s| p.status(s), |s| p.can_open(s))
            };
            if let Some(s) = clicked {
                self.goto(&ctx, s);
            }
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if let Some(id) = self.project.active_panel {
                    if self.stage.per_panel() || self.stage == Stage::Export {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let name = self.project.panel(id).map(|p| p.name.clone()).unwrap_or_default();
                            ui.label(egui::RichText::new(format!("Panel: {name}")).weak());
                        });
                    }
                }
            });
        });
        egui::Panel::right("side").resizable(true).default_size(340.0).size_range(280.0..=560.0).show(ui, |ui| {
            self.sidebar(ui, &ctx);
        });
        let canvas_rect = egui::CentralPanel::default()
            .show(ui, |ui| {
                self.panel_tabs(ui, &ctx);
                self.canvas_area(ui, &ctx);
            })
            .response
            .rect;
        self.show_dialog(&ctx);
        let hold_tip = self.is_busy() || self.dialog.is_some();
        self.tips.show(&ctx, self.stage, canvas_rect, hold_tip);
        self.global_keys(&ctx, typing);
        self.busy_overlay(&ctx);
        self.track_history(&ctx);
    }
}

impl Plot2CsvApp {
    /// Tabs for switching the active panel in per-panel stages.
    fn panel_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !(self.stage.per_panel() || self.stage == Stage::Export) {
            return;
        }
        let panels: Vec<(u32, String, bool)> =
            self.project.panel_list().iter().filter(|p| p.enabled).map(|p| (p.id, p.name.clone(), true)).collect();
        if panels.len() < 2 {
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Panel:");
            for (id, name, _) in panels {
                let st = self.project.work(id).map(|w| w.status(self.stage)).unwrap_or_default();
                let (col, _) = widgets::status_badge(st);
                let sel = self.project.active_panel == Some(id);
                let r = ui.add(egui::Button::new(egui::RichText::new(&name)).selected(sel));
                ui.painter().circle_filled(r.rect.right_top() + Vec2::new(-4.0, 4.0), 3.0, col);
                if r.clicked() && !sel {
                    self.project.active_panel = Some(id);
                    self.on_panel_switched(ctx);
                }
            }
        });
    }

    fn on_panel_switched(&mut self, ctx: &egui::Context) {
        self.axes_ui = AxesUi::default();
        self.data_ui = DataUi::default();
        self.text_ui = TextUi::default();
        // Run the pending auto step for this panel's current stage.
        if let Some(prev) = self.stage.prev() {
            let prev_ok = self.project.status(prev) == StageStatus::Approved || prev == Stage::Region;
            if prev_ok && self.project.status(self.stage) == StageStatus::Pending {
                self.run_auto(ctx, self.stage);
            }
        }
        if !self.project.can_open(self.stage) {
            // Walk back to the first stage this panel can open.
            let mut s = self.stage;
            while !self.project.can_open(s) {
                s = s.prev().unwrap_or(Stage::Import);
            }
            self.stage = s;
            if self.project.status(s) == StageStatus::Pending {
                self.run_auto(ctx, s);
            }
        }
    }
}
