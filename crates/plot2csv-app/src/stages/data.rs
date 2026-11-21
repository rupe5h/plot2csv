//! ⑥ Data: datasets, WebPlotDigitizer's manual and automatic tools, auto-detection.

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, TextureHandle};
use plot2csv_core::dataset::{Algorithm, ColorMode, DataPoint, Dataset, Mask, SeriesStyle, PALETTE};
use plot2csv_core::geom::RectF;
use plot2csv_core::project::Stage;
use plot2csv_vision::color::ColorCount;

use super::region::to_rect;
use super::{arrow_nudge, pos, pt};
use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::dialogs::Dialog;
use crate::images::bitmap_texture;
use crate::widgets::{self, c32};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum DataMode {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tool {
    #[default]
    None,
    Add,
    Adjust,
    Delete,
    MaskBox,
    MaskPen,
    MaskErase,
    PickFg,
    PickBg,
}

struct MaskEdit {
    key: (u32, usize),
    w: usize,
    h: usize,
    bits: Vec<bool>,
    tex: Option<TextureHandle>,
}

pub struct DataUi {
    pub mode: DataMode,
    pub tool: Tool,
    pub selected: Option<usize>,
    dragging: bool,
    pub focus: Option<Pos2>,
    pub pen_width: f32,
    mask: Option<MaskEdit>,
    stroke: Vec<Pos2>,
    box_start: Option<Pos2>,
    box_cur: Option<Pos2>,
    filter_tex: Option<TextureHandle>,
    pub show_mask: bool,
    pub show_filter: bool,
    dominant: Option<(u32, Vec<ColorCount>)>,
    rename: Option<(usize, String)>,
}

impl Default for DataUi {
    fn default() -> Self {
        Self {
            mode: DataMode::Auto,
            tool: Tool::None,
            selected: None,
            dragging: false,
            focus: None,
            pen_width: 14.0,
            mask: None,
            stroke: Vec::new(),
            box_start: None,
            box_cur: None,
            filter_tex: None,
            show_mask: false,
            show_filter: false,
            dominant: None,
            rename: None,
        }
    }
}

impl DataUi {
    pub fn invalidate_caches(&mut self) {
        self.mask = None;
        self.filter_tex = None;
        self.dominant = None;
    }

    pub fn pans_with_primary(&self) -> bool {
        matches!(self.tool, Tool::None)
    }
}

impl Plot2CsvApp {
    pub fn auto_data(&mut self, _ctx: &egui::Context) {
        let Some(id) = self.project.active_panel else { return };
        let Some(panel) = self.project.panel(id).cloned() else { return };
        let Some(img) = self.imgs.corrected.clone() else { return };
        let items = self.project.work(id).and_then(|w| w.text.value.clone()).unwrap_or_default();
        let cal = self.active_calibration();
        let r = plot2csv_vision::series::detect_series(&img, &panel, &items, cal.as_ref());
        let mut ds = r.datasets;
        if ds.is_empty() {
            let mut d = Dataset::new("Default Dataset");
            d.color = PALETTE[0];
            ds.push(d);
            self.data_ui.mode = DataMode::Manual;
            self.data_ui.tool = Tool::Add;
        }
        let w = self.project.work_mut(id);
        w.active_dataset = 0;
        w.data.propose(ds, r.note);
        self.data_ui = DataUi { mode: self.data_ui.mode, tool: self.data_ui.tool, ..Default::default() };
    }

    pub fn datasets(&self) -> Vec<Dataset> {
        self.project.active_work().and_then(|w| w.data.value.clone()).unwrap_or_default()
    }

    pub fn active_ds_index(&self) -> usize {
        self.project.active_work().map(|w| w.active_dataset).unwrap_or(0)
    }

    pub fn datasets_mut(&mut self) -> Option<&mut Vec<Dataset>> {
        let id = self.project.active_panel?;
        Some(self.project.work_mut(id).data.value.get_or_insert_with(Vec::new))
    }

    pub fn active_ds_mut(&mut self) -> Option<&mut Dataset> {
        let i = self.active_ds_index();
        let v = self.datasets_mut()?;
        if v.is_empty() {
            let mut d = Dataset::new("Default Dataset");
            d.color = PALETTE[0];
            v.push(d);
        }
        let n = v.len();
        v.get_mut(i.min(n - 1))
    }

    pub fn set_active_ds(&mut self, i: usize) {
        if let Some(id) = self.project.active_panel {
            self.project.work_mut(id).active_dataset = i;
        }
        self.data_ui.selected = None;
        self.data_ui.mask = None;
        self.data_ui.filter_tex = None;
    }

    pub fn add_dataset(&mut self, name: Option<String>) {
        let n = self.datasets().len();
        let fg = self.datasets().get(self.active_ds_index()).map(|d| d.auto.clone());
        if let Some(v) = self.datasets_mut() {
            let mut d = Dataset::new(name.unwrap_or_else(|| format!("Dataset {}", n + 1)));
            d.color = PALETTE[n % PALETTE.len()];
            if let Some(a) = fg {
                d.auto.algorithm = a.algorithm;
                d.auto.distance = a.distance;
            }
            v.push(d);
        }
        self.set_active_ds(n);
        self.edited(Stage::Data);
    }

    fn panel_inner(&self) -> Option<RectF> {
        self.project.active_panel.and_then(|id| self.project.panel(id)).map(|p| p.inner)
    }

    /// Bitmap of the active dataset's mask (full corrected image), creating the edit cache.
    fn mask_edit(&mut self) -> Option<&mut MaskEdit> {
        let id = self.project.active_panel?;
        let di = self.active_ds_index();
        let (w, h) = (self.imgs.corrected_size().x as usize, self.imgs.corrected_size().y as usize);
        if w == 0 {
            return None;
        }
        if self.data_ui.mask.as_ref().map(|m| m.key != (id, di) || m.w != w || m.h != h).unwrap_or(true) {
            let bits = self
                .datasets()
                .get(di)
                .and_then(|d| d.auto.mask.as_ref())
                .filter(|m| m.width as usize == w && m.height as usize == h)
                .map(|m| m.to_bits())
                .unwrap_or_else(|| vec![false; w * h]);
            self.data_ui.mask = Some(MaskEdit { key: (id, di), w, h, bits, tex: None });
        }
        self.data_ui.mask.as_mut()
    }

    fn commit_mask(&mut self, ctx: &egui::Context) {
        let Some(m) = self.mask_edit() else { return };
        let any = m.bits.iter().any(|b| *b);
        let mask = any.then(|| Mask::from_bits(m.w as u32, m.h as u32, &m.bits));
        m.tex = None;
        if let Some(d) = self.active_ds_mut() {
            d.auto.mask = mask;
        }
        self.data_ui.filter_tex = None;
        self.edited(Stage::Data);
        self.ensure_mask_texture(ctx);
    }

    fn ensure_mask_texture(&mut self, ctx: &egui::Context) {
        if let Some(m) = self.mask_edit() {
            if m.tex.is_none() {
                let bits = &m.bits;
                m.tex = Some(bitmap_texture(ctx, "mask", m.w, m.h, |i| if bits[i] { [255, 230, 0, 120] } else { [0, 0, 0, 0] }));
            }
        }
    }

    /// Pixels selected by the active dataset's colour settings within its mask
    /// (or the panel's data area when there is no mask).
    fn selected_bits(&mut self) -> Option<(Vec<bool>, usize, usize)> {
        let img = self.imgs.corrected.clone()?;
        let ds = self.datasets().get(self.active_ds_index()).cloned()?;
        let inner = self.panel_inner();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mask_bits = ds.auto.mask.as_ref().filter(|m| m.width as usize == w && m.height as usize == h).map(|m| m.to_bits());
        let region = if mask_bits.is_some() { None } else { inner.map(|r| RectF::new(r.x0 + 3.0, r.y0 + 3.0, r.x1 - 3.0, r.y1 - 3.0)) };
        let bits = plot2csv_vision::color::filter_colors(&img, &ds.auto, mask_bits.as_deref(), region);
        Some((bits, w, h))
    }

    fn run_extraction(&mut self) {
        let Some((bits, w, h)) = self.selected_bits() else { return };
        let Some(ds) = self.datasets().get(self.active_ds_index()).cloned() else { return };
        let cal = self.active_calibration();
        let bm = plot2csv_vision::extract::Bitmap { bits: &bits, width: w, height: h };
        let res = match &ds.auto.algorithm {
            Algorithm::MultiTrack { max_jump, max_gap, .. } => {
                let region = self.panel_inner().unwrap_or(RectF::new(0.0, 0.0, w as f64, h as f64));
                let tracks = plot2csv_vision::series::track_curves(&bm, &region, *max_jump, *max_gap);
                let best = tracks.into_iter().max_by(|a, b| a.pts.len().cmp(&b.pts.len()));
                Ok(best.map(|t| t.pts.iter().step_by(6).map(|&(x, y)| DataPoint::new(x, y)).collect()).unwrap_or_default())
            }
            alg => plot2csv_vision::extract::run(alg, &bm, cal.as_ref()),
        };
        match res {
            Ok(mut pts) => {
                if matches!(ds.auto.algorithm, Algorithm::Bar { .. }) {
                    let items = self.project.active_work().and_then(|w| w.text.value.clone()).unwrap_or_default();
                    let inner = self.panel_inner().unwrap_or_default();
                    for p in pts.iter_mut() {
                        p.label = items
                            .iter()
                            .filter(|it| it.bbox.center().y > inner.y1 && it.value.is_none())
                            .map(|it| ((it.bbox.center().x - p.x).abs(), it))
                            .filter(|(d, it)| *d < it.bbox.width().max(20.0))
                            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
                            .map(|(_, it)| it.text.clone());
                    }
                }
                let n = pts.len();
                if let Some(d) = self.active_ds_mut() {
                    d.points = pts;
                }
                self.status = format!("{} found {n} points.", ds.auto.algorithm.label());
                self.edited(Stage::Data);
            }
            Err(e) => self.status = e,
        }
    }

    fn build_filter_texture(&mut self, ctx: &egui::Context) {
        let Some((bits, w, h)) = self.selected_bits() else { return };
        let mask = self
            .datasets()
            .get(self.active_ds_index())
            .and_then(|d| d.auto.mask.clone())
            .filter(|m| m.width as usize == w && m.height as usize == h)
            .map(|m| m.to_bits());
        let inner = self.panel_inner();
        let in_region = |i: usize| -> bool {
            match &mask {
                Some(m) => m[i],
                None => inner.map(|r| r.contains(plot2csv_core::geom::Pt::new((i % w) as f64, (i / w) as f64))).unwrap_or(true),
            }
        };
        self.data_ui.filter_tex = Some(bitmap_texture(ctx, "filter", w, h, |i| {
            if bits[i] {
                [255, 230, 0, 255]
            } else if in_region(i) {
                [90, 90, 90, 200]
            } else {
                [0, 0, 0, 0]
            }
        }));
    }

    fn dominant_colors(&mut self) -> Vec<ColorCount> {
        let id = self.project.active_panel.unwrap_or(0);
        if self.data_ui.dominant.as_ref().map(|d| d.0 != id).unwrap_or(true) {
            let cols = match &self.imgs.corrected {
                Some(img) => plot2csv_vision::color::dominant_colors(img, self.panel_inner(), 10, 12.0),
                None => Vec::new(),
            };
            self.data_ui.dominant = Some((id, cols));
        }
        self.data_ui.dominant.as_ref().map(|d| d.1.clone()).unwrap_or_default()
    }

    // ------------------------------------------------------------- sidebar

    pub fn data_sidebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        widgets::section(ui, "⑥ Data series");
        if let Some(w) = self.project.active_work() {
            if !w.data.note.is_empty() {
                ui.label(egui::RichText::new(&w.data.note).color(widgets::INFO));
            }
        }
        if self.active_calibration().is_none() {
            ui.label(egui::RichText::new("Axes are not calibrated: values will be pixels.").color(widgets::WARN));
        }
        self.datasets_list(ui);
        ui.separator();
        // Mode toggle (WPD: Automatic Mode / Manual Mode)
        ui.horizontal(|ui| {
            if ui.selectable_label(self.data_ui.mode == DataMode::Auto, egui::RichText::new("Automatic Mode").strong()).clicked() {
                self.data_ui.mode = DataMode::Auto;
                self.data_ui.tool = Tool::None;
            }
            if ui.selectable_label(self.data_ui.mode == DataMode::Manual, egui::RichText::new("Manual Mode").strong()).clicked() {
                self.data_ui.mode = DataMode::Manual;
                self.data_ui.tool = Tool::Add;
            }
        });
        match self.data_ui.mode {
            DataMode::Manual => self.manual_tools(ui),
            DataMode::Auto => self.auto_tools(ui, ctx),
        }
        ui.add_space(6.0);
        let n = self.datasets().get(self.active_ds_index()).map(|d| d.points.len()).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label(format!("Data Points: {n}"));
            if ui.button("View Data").clicked() {
                self.dialog = Some(Dialog::AcquiredData);
            }
        });
    }

    fn datasets_list(&mut self, ui: &mut egui::Ui) {
        let dss = self.datasets();
        let active = self.active_ds_index();
        let mut changed = false;
        let mut new_active = None;
        let mut delete = None;
        let mut updates: Vec<(usize, Dataset)> = Vec::new();
        for (i, d) in dss.iter().enumerate() {
            let mut d2 = d.clone();
            ui.horizontal(|ui| {
                if ui.radio(i == active, "").clicked() {
                    new_active = Some(i);
                }
                let mut col = egui::Color32::from_rgb(d.color[0], d.color[1], d.color[2]);
                if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed() {
                    d2.color = [col.r(), col.g(), col.b()];
                }
                match &mut self.data_ui.rename {
                    Some((ri, name)) if *ri == i => {
                        let r = ui.add(egui::TextEdit::singleline(name).desired_width(110.0));
                        if r.lost_focus() {
                            d2.name = name.clone();
                            self.data_ui.rename = None;
                        } else {
                            r.request_focus();
                        }
                    }
                    _ => {
                        let r = ui.selectable_label(i == active, &d.name).on_hover_text("Double-click to rename");
                        if r.clicked() {
                            new_active = Some(i);
                        }
                        if r.double_clicked() {
                            self.data_ui.rename = Some((i, d.name.clone()));
                        }
                    }
                }
                ui.label(egui::RichText::new(format!("{}", d.points.len())).weak());
                ui.checkbox(&mut d2.visible, "").on_hover_text("Show on image");
                if ui.small_button("🗑").on_hover_text("Delete dataset").clicked() {
                    delete = Some(i);
                }
            });
            if d2 != *d {
                updates.push((i, d2));
            }
        }
        for (i, d) in updates {
            if let Some(v) = self.datasets_mut() {
                v[i] = d;
            }
            changed = true;
        }
        if let Some(i) = delete {
            if let Some(v) = self.datasets_mut() {
                v.remove(i);
            }
            self.set_active_ds(active.min(dss.len().saturating_sub(2)));
            changed = true;
        }
        if let Some(i) = new_active {
            self.set_active_ds(i);
        }
        ui.horizontal(|ui| {
            if ui.button("＋ Add dataset").clicked() {
                self.add_dataset(None);
            }
            if ui.button("Manage…").clicked() {
                self.dialog = Some(Dialog::Datasets);
            }
        });
        if changed {
            self.edited(Stage::Data);
        }
    }

    fn manual_tools(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (t, label) in [(Tool::Add, "Add Point (A)"), (Tool::Adjust, "Adjust Point (S)"), (Tool::Delete, "Delete Point (D)")] {
                if ui.selectable_label(self.data_ui.tool == t, label).clicked() {
                    self.data_ui.tool = t;
                }
            }
            if ui.button("Clear Points").clicked() {
                if let Some(d) = self.active_ds_mut() {
                    d.points.clear();
                }
                self.data_ui.selected = None;
                self.edited(Stage::Data);
            }
        });
        let hint = match self.data_ui.tool {
            Tool::Add => "Click on the curve to add points.",
            Tool::Adjust => "Click a point to select it; drag it, or use the arrow keys (Shift ×10). Q / W: next / previous point.",
            Tool::Delete => "Click points to delete them.",
            _ => "",
        };
        ui.label(egui::RichText::new(hint).weak());
        if self.data_ui.tool == Tool::Add {
            if let Some(d) = self.datasets().get(self.active_ds_index()) {
                if d.style == SeriesStyle::Bars {
                    ui.label(egui::RichText::new("Bar points get the label of the category text under them.").weak().small());
                }
            }
        }
    }

    fn auto_tools(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(mut ds) = self.datasets().get(self.active_ds_index()).cloned() else {
            if ui.button("＋ Add dataset").clicked() {
                self.add_dataset(None);
            }
            return;
        };
        let before = ds.auto.clone();
        // --- Mask
        egui::Grid::new("auto_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(egui::RichText::new("Mask").strong());
            ui.horizontal_wrapped(|ui| {
                for (t, label, tip) in [
                    (Tool::MaskBox, "Box", "Drag a box to include an area"),
                    (Tool::MaskPen, "Pen", "Paint the area of interest"),
                    (Tool::MaskErase, "Erase", "Erase parts of the mask"),
                ] {
                    if ui.selectable_label(self.data_ui.tool == t, label).on_hover_text(tip).clicked() {
                        self.data_ui.tool = if self.data_ui.tool == t { Tool::None } else { t };
                        self.data_ui.show_mask = true;
                    }
                }
                ui.toggle_value(&mut self.data_ui.show_mask, "View");
                if ui.small_button("Erase All").clicked() {
                    ds.auto.mask = None;
                    self.data_ui.mask = None;
                }
            });
            ui.end_row();
            if matches!(self.data_ui.tool, Tool::MaskPen | Tool::MaskErase) {
                ui.label("Stroke");
                ui.add(egui::Slider::new(&mut self.data_ui.pen_width, 2.0..=80.0).suffix(" px"));
                ui.end_row();
            }
            // --- Colour
            ui.label(egui::RichText::new("Color").strong());
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("cmode")
                    .selected_text(match ds.auto.color_mode {
                        ColorMode::Foreground => "Foreground Color",
                        ColorMode::Background => "Background Color",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut ds.auto.color_mode, ColorMode::Foreground, "Foreground Color");
                        ui.selectable_value(&mut ds.auto.color_mode, ColorMode::Background, "Background Color");
                    });
                let c = if ds.auto.color_mode == ColorMode::Foreground { ds.auto.fg } else { ds.auto.bg };
                if widgets::swatch(ui, c, 20.0).on_hover_text("Click to change the colour").clicked() {
                    self.dialog = Some(Dialog::ColorPicker { background: ds.auto.color_mode == ColorMode::Background });
                }
                let pick = if ds.auto.color_mode == ColorMode::Foreground { Tool::PickFg } else { Tool::PickBg };
                if ui.selectable_label(self.data_ui.tool == pick, "🖊 Pick").on_hover_text("Pick the colour from the image").clicked() {
                    self.data_ui.tool = pick;
                }
            });
            ui.end_row();
            ui.label("Distance");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut ds.auto.distance).range(1.0..=442.0).speed(1.0));
                if ui.toggle_value(&mut self.data_ui.show_filter, "Filter Colors").on_hover_text("Show which pixels match").clicked() && self.data_ui.show_filter {
                    self.data_ui.filter_tex = None;
                }
            });
            ui.end_row();
        });
        // Dominant colours
        let dom = self.dominant_colors();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Dominant:").small());
            for c in dom.iter().take(10) {
                if widgets::swatch(ui, c.color, 16.0).on_hover_text(format!("RGB {:?}", c.color)).clicked() {
                    if ds.auto.color_mode == ColorMode::Foreground {
                        ds.auto.fg = c.color;
                    } else {
                        ds.auto.bg = c.color;
                    }
                }
            }
        });
        // --- Algorithm
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Algorithm").strong());
            let (dx_def, step_def) = (10.0, self.default_xstep());
            egui::ComboBox::from_id_salt("alg").selected_text(ds.auto.algorithm.label()).width(170.0).show_ui(ui, |ui| {
                ui.label(egui::RichText::new("Generic").weak());
                let opts: Vec<Algorithm> = vec![
                    Algorithm::AveragingWindow { dx: dx_def, dy: dx_def },
                    Algorithm::XStepInterp { xmin: step_def.0, xstep: step_def.1, xmax: step_def.2, ymin: step_def.3, ymax: step_def.4, smoothing: 0.0 },
                    Algorithm::XStep { xmin: step_def.0, xstep: step_def.1, xmax: step_def.2, ymin: step_def.3, ymax: step_def.4 },
                    Algorithm::MultiTrack { tracks: 1, max_jump: 10.0, max_gap: 20.0 },
                ];
                for o in opts {
                    let sel = std::mem::discriminant(&o) == std::mem::discriminant(&ds.auto.algorithm);
                    if ui.selectable_label(sel, o.label()).clicked() && !sel {
                        ds.auto.algorithm = o;
                    }
                }
                ui.label(egui::RichText::new("Blobs").weak());
                let o = Algorithm::Blob { min_d: 2.0, max_d: 30.0 };
                let sel = matches!(ds.auto.algorithm, Algorithm::Blob { .. });
                if ui.selectable_label(sel, o.label()).clicked() && !sel {
                    ds.auto.algorithm = o;
                }
                ui.label(egui::RichText::new("Bars").weak());
                let o = Algorithm::Bar { min_width: 3.0 };
                let sel = matches!(ds.auto.algorithm, Algorithm::Bar { .. });
                if ui.selectable_label(sel, o.label()).clicked() && !sel {
                    ds.auto.algorithm = o;
                }
            });
        });
        algorithm_params(ui, &mut ds.auto.algorithm);
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, "Run", true).clicked() {
                if before != ds.auto {
                    self.store_auto_settings(ds.auto.clone());
                }
                self.defer("Extracting points…", |app, _| app.run_extraction());
            }
            if ui.button("Clear Points").clicked() {
                if let Some(d) = self.active_ds_mut() {
                    d.points.clear();
                }
                self.edited(Stage::Data);
            }
        });
        if before != ds.auto {
            self.store_auto_settings(ds.auto);
            self.data_ui.filter_tex = None;
        }
        if self.data_ui.show_filter && self.data_ui.filter_tex.is_none() {
            self.build_filter_texture(ctx);
        }
        if self.data_ui.show_mask {
            self.ensure_mask_texture(ctx);
        }
    }

    fn store_auto_settings(&mut self, a: plot2csv_core::dataset::AutoSettings) {
        let mask_changed = self.datasets().get(self.active_ds_index()).map(|d| d.auto.mask != a.mask).unwrap_or(false);
        if let Some(d) = self.active_ds_mut() {
            d.auto = a;
        }
        if mask_changed {
            self.data_ui.mask = None;
        }
        self.edited(Stage::Data);
    }

    /// Defaults for the X-step algorithms from the calibrated data range.
    fn default_xstep(&self) -> (f64, f64, f64, f64, f64) {
        if let (Some(cal), Some(r)) = (self.active_calibration(), self.panel_inner()) {
            let a = cal.to_data(plot2csv_core::geom::Pt::new(r.x0, r.y1));
            let b = cal.to_data(plot2csv_core::geom::Pt::new(r.x1, r.y0));
            if a.len() >= 2 && b.len() >= 2 {
                let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
                let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
                let step = super::axes::nice_ticks(x0, x1, 50).windows(2).next().map(|w| w[1] - w[0]).unwrap_or((x1 - x0) / 50.0);
                return (x0, step, x1, y0, y1);
            }
        }
        (0.0, 0.1, 1.0, 0.0, 1.0)
    }

    // -------------------------------------------------------------- canvas

    pub fn data_canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, view: &CanvasView, interactive: bool) {
        let painter = ui.painter_at(view.rect);
        let img_rect = view.rect_to_screen(Rect::from_min_size(Pos2::ZERO, self.imgs.corrected_size()));
        if let Some(r) = self.panel_inner() {
            painter.rect_stroke(view.rect_to_screen(to_rect(&r)), 0.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 140, 255, 70)), StrokeKind::Middle);
        }
        // Filter / mask overlays.
        if self.data_ui.show_filter {
            if let Some(t) = &self.data_ui.filter_tex {
                painter.image(t.id(), img_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        } else if self.data_ui.show_mask {
            if let Some(t) = self.data_ui.mask.as_ref().and_then(|m| m.tex.as_ref()) {
                painter.image(t.id(), img_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
        // Datasets.
        let dss = self.datasets();
        let active = self.active_ds_index();
        for (i, d) in dss.iter().enumerate() {
            if !d.visible && i != active {
                continue;
            }
            let col = c32(d.color);
            let r = if i == active { 4.0 } else { 2.5 };
            let outline = if i == active { Stroke::new(1.0, Color32::BLACK) } else { Stroke::new(0.5, Color32::from_black_alpha(120)) };
            for (k, p) in d.points.iter().enumerate() {
                let s = view.to_screen(pos(p.pt()));
                if !view.rect.expand(5.0).contains(s) {
                    continue;
                }
                let sel = i == active && self.data_ui.selected == Some(k);
                if sel {
                    painter.circle_filled(s, r + 4.0, Color32::from_rgba_unmultiplied(0, 255, 0, 140));
                }
                painter.circle_filled(s, r, col);
                painter.circle_stroke(s, r, outline);
                if let (Some(l), true) = (&p.label, i == active) {
                    painter.text(s + egui::vec2(5.0, -5.0), egui::Align2::LEFT_BOTTOM, l, egui::FontId::proportional(11.0), col);
                }
            }
        }
        // Live mask stroke / box preview.
        if !self.data_ui.stroke.is_empty() {
            let col = if self.data_ui.tool == Tool::MaskErase { Color32::from_rgba_unmultiplied(255, 255, 255, 160) } else { Color32::from_rgba_unmultiplied(255, 230, 0, 140) };
            let pts: Vec<Pos2> = self.data_ui.stroke.iter().map(|p| view.to_screen(*p)).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(self.data_ui.pen_width * view.zoom, col)));
        }
        if let (Some(a), Some(b)) = (self.data_ui.box_start, self.data_ui.box_cur) {
            let r = Rect::from_two_pos(view.to_screen(a), view.to_screen(b));
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 230, 0, 90));
        }
        // Pen cursor.
        if let (Some(h), true) = (view.hover, matches!(self.data_ui.tool, Tool::MaskPen | Tool::MaskErase)) {
            painter.circle_stroke(view.to_screen(h), self.data_ui.pen_width * view.zoom / 2.0, Stroke::new(1.0, Color32::BLACK));
        }
        if !interactive || self.dialog.is_some() {
            return;
        }
        self.data_keys(ctx);
        let resp = &view.response;
        let pointer = resp.interact_pointer_pos();
        let ip = pointer.map(|p| view.to_image(p));
        let nearest = |dss: &[Dataset], p: Pos2| -> Option<usize> {
            let d = dss.get(active)?;
            d.nearest(pt(p), 12.0 / view.zoom as f64)
        };
        match self.data_ui.tool {
            Tool::Add => {
                if resp.clicked() {
                    if let Some(p) = ip {
                        let label = self.bar_label_at(p);
                        if let Some(d) = self.active_ds_mut() {
                            d.points.push(DataPoint { x: p.x as f64, y: p.y as f64, label });
                            let n = d.points.len() - 1;
                            self.data_ui.selected = Some(n);
                        }
                        self.edited(Stage::Data);
                    }
                }
            }
            Tool::Delete => {
                if resp.clicked() {
                    if let Some(k) = ip.and_then(|p| nearest(&dss, p)) {
                        if let Some(d) = self.active_ds_mut() {
                            d.points.remove(k);
                        }
                        self.data_ui.selected = None;
                        self.edited(Stage::Data);
                    }
                }
            }
            Tool::Adjust => {
                if resp.clicked() {
                    self.data_ui.selected = ip.and_then(|p| nearest(&dss, p));
                }
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let k = ip.and_then(|p| nearest(&dss, p));
                    self.data_ui.dragging = k.is_some();
                    if k.is_some() {
                        self.data_ui.selected = k;
                    }
                }
                if self.data_ui.dragging && resp.dragged_by(egui::PointerButton::Primary) {
                    if let (Some(k), Some(p)) = (self.data_ui.selected, ip) {
                        if let Some(d) = self.active_ds_mut() {
                            if let Some(q) = d.points.get_mut(k) {
                                q.x = p.x as f64;
                                q.y = p.y as f64;
                            }
                        }
                        self.data_ui.focus = Some(p);
                        self.edited(Stage::Data);
                    }
                }
                if resp.drag_stopped() {
                    self.data_ui.dragging = false;
                }
            }
            Tool::MaskBox => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    self.data_ui.box_start = ip;
                }
                if resp.dragged_by(egui::PointerButton::Primary) {
                    self.data_ui.box_cur = ip;
                }
                if resp.drag_stopped() {
                    if let (Some(a), Some(b)) = (self.data_ui.box_start.take(), self.data_ui.box_cur.take()) {
                        if let Some(m) = self.mask_edit() {
                            let r = Rect::from_two_pos(a, b);
                            for y in (r.top().max(0.0) as usize)..(r.bottom().min(m.h as f32) as usize) {
                                for x in (r.left().max(0.0) as usize)..(r.right().min(m.w as f32) as usize) {
                                    m.bits[y * m.w + x] = true;
                                }
                            }
                        }
                        self.commit_mask(ctx);
                    }
                }
            }
            Tool::MaskPen | Tool::MaskErase => {
                if resp.drag_started_by(egui::PointerButton::Primary) || resp.dragged_by(egui::PointerButton::Primary) {
                    if let Some(p) = ip {
                        self.data_ui.stroke.push(p);
                    }
                }
                let click = resp.clicked();
                if click {
                    if let Some(p) = ip {
                        self.data_ui.stroke.push(p);
                    }
                }
                if (resp.drag_stopped() || click) && !self.data_ui.stroke.is_empty() {
                    let stroke = std::mem::take(&mut self.data_ui.stroke);
                    let erase = self.data_ui.tool == Tool::MaskErase;
                    let r = self.data_ui.pen_width / 2.0;
                    if let Some(m) = self.mask_edit() {
                        paint_stroke(&mut m.bits, m.w, m.h, &stroke, r, !erase);
                    }
                    self.commit_mask(ctx);
                }
            }
            Tool::PickFg | Tool::PickBg => {
                if resp.clicked() {
                    if let (Some(p), Some(img)) = (ip, self.imgs.corrected.clone()) {
                        let c = sample_color(&img, p);
                        let bg = self.data_ui.tool == Tool::PickBg;
                        if let Some(d) = self.active_ds_mut() {
                            if bg {
                                d.auto.bg = c;
                            } else {
                                d.auto.fg = c;
                                d.color = c;
                            }
                        }
                        self.data_ui.filter_tex = None;
                        self.data_ui.tool = Tool::None;
                        self.edited(Stage::Data);
                    }
                }
            }
            Tool::None => {}
        }
        if view.hover.is_some() && !self.data_ui.dragging {
            self.data_ui.focus = None;
        }
    }

    /// Category label (text under the plot) for a bar point added by hand.
    fn bar_label_at(&self, p: Pos2) -> Option<String> {
        let ds = self.datasets();
        let d = ds.get(self.active_ds_index())?;
        let kind = self.project.active_panel.and_then(|id| self.project.panel(id)).map(|q| q.kind);
        if d.style != SeriesStyle::Bars && kind != Some(plot2csv_core::AxesKind::Bar) {
            return None;
        }
        let inner = self.panel_inner()?;
        let items = self.project.active_work()?.text.value.clone()?;
        items
            .iter()
            .filter(|it| it.bbox.center().y > inner.y1 && it.value.is_none())
            .map(|it| ((it.bbox.center().x - p.x as f64).abs(), it))
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(_, it)| it.text.clone())
    }

    fn data_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (a, s, d, q, w, del) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::D),
                i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::W),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if a || s || d {
            self.data_ui.mode = DataMode::Manual;
            self.data_ui.tool = if a {
                Tool::Add
            } else if s {
                Tool::Adjust
            } else {
                Tool::Delete
            };
        }
        let n = self.datasets().get(self.active_ds_index()).map(|d| d.points.len()).unwrap_or(0);
        if n > 0 && (q || w) {
            let cur = self.data_ui.selected.unwrap_or(0);
            let k = if q { (cur + 1) % n } else { (cur + n - 1) % n };
            self.data_ui.selected = Some(k);
            self.data_ui.tool = Tool::Adjust;
            self.data_ui.focus = self.datasets().get(self.active_ds_index()).and_then(|d| d.points.get(k)).map(|p| pos(p.pt()));
        }
        if let Some(k) = self.data_ui.selected {
            if let Some(dv) = arrow_nudge(ctx) {
                if let Some(ds) = self.active_ds_mut() {
                    if let Some(p) = ds.points.get_mut(k) {
                        p.x += dv.x as f64;
                        p.y += dv.y as f64;
                        let f = pos(p.pt());
                        self.data_ui.focus = Some(f);
                    }
                }
                self.edited(Stage::Data);
            }
            if del && self.data_ui.tool == Tool::Adjust {
                if let Some(ds) = self.active_ds_mut() {
                    if k < ds.points.len() {
                        ds.points.remove(k);
                    }
                }
                self.data_ui.selected = None;
                self.edited(Stage::Data);
            }
        }
    }
}

fn sample_color(img: &image::RgbaImage, p: Pos2) -> [u8; 3] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (cx, cy) = (p.x as i64, p.y as i64);
    // The darkest-ish (least background) pixel in a 5x5 window: thin lines are easy to hit.
    let mut best = ([255u8; 3], -1i64);
    for dy in -2..=2 {
        for dx in -2..=2 {
            let (x, y) = (cx + dx, cy + dy);
            if x >= 0 && y >= 0 && x < w && y < h {
                let c = img.get_pixel(x as u32, y as u32).0;
                let score = 765 - (c[0] as i64 + c[1] as i64 + c[2] as i64) + plot2csv_vision::color::chroma([c[0], c[1], c[2]]) as i64 * 4;
                if score > best.1 {
                    best = ([c[0], c[1], c[2]], score);
                }
            }
        }
    }
    best.0
}

fn paint_stroke(bits: &mut [bool], w: usize, h: usize, stroke: &[Pos2], r: f32, value: bool) {
    let mut dab = |c: Pos2| {
        let r2 = r * r;
        for y in ((c.y - r).floor().max(0.0) as usize)..((c.y + r).ceil().min(h as f32) as usize) {
            for x in ((c.x - r).floor().max(0.0) as usize)..((c.x + r).ceil().min(w as f32) as usize) {
                let (dx, dy) = (x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y);
                if dx * dx + dy * dy <= r2 {
                    bits[y * w + x] = value;
                }
            }
        }
    };
    for (i, p) in stroke.iter().enumerate() {
        dab(*p);
        if let Some(q) = stroke.get(i + 1) {
            let n = (p.distance(*q) / (r * 0.5).max(1.0)).ceil() as usize;
            for k in 1..n {
                dab(p.lerp(*q, k as f32 / n as f32));
            }
        }
    }
}

/// Parameter fields for the selected algorithm.
fn algorithm_params(ui: &mut egui::Ui, alg: &mut Algorithm) {
    let num = |ui: &mut egui::Ui, label: &str, v: &mut f64, unit: &str, speed: f64| {
        ui.label(label);
        ui.add(egui::DragValue::new(v).speed(speed).max_decimals(6));
        ui.label(egui::RichText::new(unit).weak());
        ui.end_row();
    };
    egui::Grid::new("alg_params").num_columns(3).show(ui, |ui| match alg {
        Algorithm::AveragingWindow { dx, dy } => {
            num(ui, "ΔX", dx, "Px", 0.5);
            num(ui, "ΔY", dy, "Px", 0.5);
        }
        Algorithm::XStepInterp { xmin, xstep, xmax, ymin, ymax, smoothing } => {
            let sp = (*xstep).abs().max(1e-9) * 0.1;
            num(ui, "X_min", xmin, "Units", sp);
            num(ui, "ΔX Step", xstep, "Units", sp * 0.1);
            num(ui, "X_max", xmax, "Units", sp);
            num(ui, "Y_min", ymin, "Units", sp);
            num(ui, "Y_max", ymax, "Units", sp);
            num(ui, "Smoothing", smoothing, "% of ΔX", 1.0);
        }
        Algorithm::XStep { xmin, xstep, xmax, ymin, ymax } => {
            let sp = (*xstep).abs().max(1e-9) * 0.1;
            num(ui, "X_min", xmin, "Units", sp);
            num(ui, "ΔX Step", xstep, "Units", sp * 0.1);
            num(ui, "X_max", xmax, "Units", sp);
            num(ui, "Y_min", ymin, "Units", sp);
            num(ui, "Y_max", ymax, "Units", sp);
        }
        Algorithm::Blob { min_d, max_d } => {
            num(ui, "Min diameter", min_d, "Px", 0.5);
            num(ui, "Max diameter", max_d, "Px", 0.5);
        }
        Algorithm::Bar { min_width } => {
            num(ui, "Min bar width", min_width, "Px", 0.5);
        }
        Algorithm::MultiTrack { max_jump, max_gap, .. } => {
            num(ui, "Max jump", max_jump, "Px", 0.5);
            num(ui, "Max gap", max_gap, "Px", 0.5);
        }
    });
}
