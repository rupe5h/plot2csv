//! ⑦ Export: WebPlotDigitizer's "Acquired Data" view, CSV/ZIP/JSON export and a plot preview.

use egui::{Color32, Stroke};
use egui_plot::{Line, Plot, PlotPoints, Points};
use plot2csv_core::axes::Calibration;
use plot2csv_core::dataset::Dataset;
use plot2csv_core::export::{export_zip, long_csv, series_csv, wide_csv, SeriesRef};
use plot2csv_core::table::{format_rows, rows, SortBy};
use plot2csv_core::NumFormat;

use super::pos;
use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::platform;
use crate::widgets::{self, c32};

#[derive(Default)]
pub struct ExportUi {
    /// (panel id, dataset index)
    pub sel: Option<(u32, usize)>,
    pub plot_preview: bool,
}

/// Every dataset with a usable calibration: (panel id, panel name, calibration, dataset).
pub type SeriesOwned = (u32, String, Calibration, Dataset);

impl Plot2CsvApp {
    pub fn all_series(&self) -> Vec<SeriesOwned> {
        let mut out = Vec::new();
        for p in self.project.panel_list().iter().filter(|p| p.enabled) {
            let Some(w) = self.project.work(p.id) else { continue };
            let cal = w
                .axes
                .value
                .as_ref()
                .and_then(|a| a.calibrate().ok())
                .unwrap_or_else(|| plot2csv_core::Axes::new(plot2csv_core::AxesKind::Image, 0.0).calibrate().unwrap());
            for d in w.data.value.iter().flatten() {
                out.push((p.id, p.name.clone(), cal.clone(), d.clone()));
            }
        }
        out
    }

    fn base_name(&self) -> String {
        self.project
            .source
            .as_ref()
            .map(|s| s.name.rsplit_once('.').map(|(a, _)| a).unwrap_or(&s.name).to_string())
            .unwrap_or_else(|| "plot2csv".into())
    }

    pub fn export_zip(&mut self) {
        let all = self.all_series();
        let refs: Vec<SeriesRef> = all.iter().map(|(_, n, c, d)| SeriesRef { panel: n, cal: c, ds: d }).collect();
        let json = self.project.to_json();
        match export_zip(&refs, &self.table_opts, Some(&json)) {
            Ok(z) => {
                platform::save_file(&format!("{}_data.zip", self.base_name()), z);
                self.status = format!("Exported {} series.", refs.len());
            }
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    /// The Acquired Data table (used by the Export stage and the View Data dialog).
    pub fn acquired_data_ui(&mut self, ui: &mut egui::Ui) {
        let all = self.all_series();
        if all.is_empty() {
            ui.label("No data yet.");
            return;
        }
        // Default selection: active panel's active dataset.
        let default_sel = self.project.active_panel.map(|id| (id, self.active_ds_index()));
        let mut sel = self.export_ui.sel.or(default_sel).unwrap_or((all[0].0, 0));
        if !all.iter().any(|s| s.0 == sel.0) {
            sel = (all[0].0, 0);
        }
        let in_panel: Vec<&SeriesOwned> = all.iter().filter(|s| s.0 == sel.0).collect();
        sel.1 = sel.1.min(in_panel.len().saturating_sub(1));
        let multi_panel = all.iter().map(|s| s.0).collect::<std::collections::HashSet<_>>().len() > 1;
        ui.horizontal(|ui| {
            ui.label("Dataset:");
            let cur = in_panel.get(sel.1).map(|s| if multi_panel { format!("{} / {}", s.1, s.3.name) } else { s.3.name.clone() }).unwrap_or_default();
            egui::ComboBox::from_id_salt("acq_ds").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                let mut idx_in_panel = std::collections::HashMap::new();
                for s in &all {
                    let k = idx_in_panel.entry(s.0).or_insert(0usize);
                    let label = if multi_panel { format!("{} / {}", s.1, s.3.name) } else { s.3.name.clone() };
                    if ui.selectable_label(sel == (s.0, *k), label).clicked() {
                        sel = (s.0, *k);
                    }
                    *k += 1;
                }
            });
        });
        self.export_ui.sel = Some(sel);
        let all_in_panel: Vec<SeriesOwned> = all.iter().filter(|s| s.0 == sel.0).cloned().collect();
        let Some((_, pname, cal, ds)) = all_in_panel.get(sel.1).cloned() else { return };
        let vars = cal.variables();
        ui.label(format!("Variables: {}", vars.join(", ")));
        let opts = &mut self.table_opts;
        egui::Grid::new("acq_opts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label(egui::RichText::new("Sort by").strong());
            ui.horizontal(|ui| {
                let label = match opts.sort_by {
                    SortBy::Raw => "Raw".to_string(),
                    SortBy::Column(c) => vars.get(c + usize::from(matches!(cal, Calibration::Bar(_)))).cloned().unwrap_or_default(),
                    SortBy::NearestNeighbor => "Nearest Neighbor".into(),
                };
                egui::ComboBox::from_id_salt("sortby").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut opts.sort_by, SortBy::Raw, "Raw");
                    let ncols = cal.to_data(plot2csv_core::geom::Pt::default()).len();
                    for c in 0..ncols {
                        let name = vars.get(c + usize::from(matches!(cal, Calibration::Bar(_)))).cloned().unwrap_or_default();
                        ui.selectable_value(&mut opts.sort_by, SortBy::Column(c), name);
                    }
                    ui.selectable_value(&mut opts.sort_by, SortBy::NearestNeighbor, "Nearest Neighbor");
                });
                ui.add_enabled_ui(matches!(opts.sort_by, SortBy::Column(_)), |ui| {
                    egui::ComboBox::from_id_salt("order").selected_text(if opts.ascending { "Ascending" } else { "Descending" }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut opts.ascending, true, "Ascending");
                        ui.selectable_value(&mut opts.ascending, false, "Descending");
                    });
                });
            });
            ui.end_row();
            ui.label(egui::RichText::new("Format").strong());
            ui.horizontal(|ui| {
                ui.label("Digits");
                ui.add(egui::DragValue::new(&mut opts.digits).range(0..=16));
                egui::ComboBox::from_id_salt("numfmt").selected_text(opts.format.label()).show_ui(ui, |ui| {
                    for f in NumFormat::ALL {
                        ui.selectable_value(&mut opts.format, f, f.label());
                    }
                });
            });
            ui.end_row();
            ui.label("Separator");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut opts.separator).desired_width(40.0)).on_hover_text("Use \\t for a tab");
                ui.checkbox(&mut opts.header, "Header row");
            });
            ui.end_row();
        });
        let r = rows(&cal, &ds, &self.table_opts);
        let mut text = format_rows(&cal, &r, &self.table_opts);
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .font(egui::TextStyle::Monospace)
                .desired_rows(10)
                .desired_width(f32::INFINITY)
                .interactive(true),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy").on_hover_text("Copy the table to the clipboard").clicked() {
                ui.ctx().copy_text(text.clone());
                self.status = "Copied to clipboard.".into();
            }
            if ui.button("Download .CSV").clicked() {
                let refd = SeriesRef { panel: &pname, cal: &cal, ds: &ds };
                let csv = if self.table_opts.header { text.clone() } else { series_csv(&refd, &self.table_opts) };
                let name = format!(
                    "{}_{}.csv",
                    plot2csv_core::export::sanitize_filename(&self.base_name()),
                    plot2csv_core::export::sanitize_filename(&ds.name)
                );
                platform::save_file(&name, csv.into_bytes());
            }
        });
    }

    pub fn export_sidebar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        widgets::section(ui, "⑦ Export all series");
        let all = self.all_series();
        ui.label(egui::RichText::new(format!("{} series in {} panel(s).", all.len(), self.project.panel_list().iter().filter(|p| p.enabled).count())).weak());
        ui.horizontal_wrapped(|ui| {
            if widgets::primary_button(ui, "⬇ Download all (ZIP)", !all.is_empty()).on_hover_text("One CSV per series, a combined long CSV, wide CSVs and the project").clicked() {
                self.export_zip();
            }
            if ui.add_enabled(!all.is_empty(), egui::Button::new("Long CSV")).on_hover_text("panel, series, values").clicked() {
                let refs: Vec<SeriesRef> = all.iter().map(|(_, n, c, d)| SeriesRef { panel: n, cal: c, ds: d }).collect();
                platform::save_file(&format!("{}_all_series.csv", self.base_name()), long_csv(&refs, &self.table_opts).into_bytes());
            }
            let pid = self.project.active_panel;
            let panel_refs: Vec<SeriesRef> = all.iter().filter(|s| Some(s.0) == pid).map(|(_, n, c, d)| SeriesRef { panel: n, cal: c, ds: d }).collect();
            let wide = wide_csv(&panel_refs, &self.table_opts);
            if ui.add_enabled(wide.is_some(), egui::Button::new("Wide CSV")).on_hover_text("Shared X column; other series interpolated (X-Y plots)").clicked() {
                if let Some(w) = wide {
                    platform::save_file(&format!("{}_wide.csv", self.base_name()), w.into_bytes());
                }
            }
            if ui.button("Project (JSON)").on_hover_text("Save everything to resume later").clicked() {
                self.save_project();
            }
        });
        ui.checkbox(&mut self.export_ui.plot_preview, "Show re-plotted data instead of the image");
        ui.separator();
        widgets::section(ui, "Acquired Data");
        self.acquired_data_ui(ui);
    }

    pub fn export_canvas(&mut self, ui: &mut egui::Ui, view: &CanvasView) {
        if self.export_ui.plot_preview {
            let all = self.all_series();
            let pid = self.project.active_panel;
            let rect = view.rect;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child.painter().rect_filled(rect, 0.0, child.visuals().panel_fill);
            Plot::new("preview").legend(egui_plot::Legend::default()).show(&mut child, |pu| {
                for (pid2, _, cal, ds) in all.iter().filter(|s| Some(s.0) == pid) {
                    let _ = pid2;
                    let mut pts: Vec<[f64; 2]> = ds
                        .points
                        .iter()
                        .map(|p| {
                            let d = cal.to_data(p.pt());
                            if d.len() >= 2 {
                                [d[0], d[1]]
                            } else {
                                [p.x, d[0]]
                            }
                        })
                        .collect();
                    let col = c32(ds.color);
                    if matches!(ds.style, plot2csv_core::SeriesStyle::Markers | plot2csv_core::SeriesStyle::Bars) {
                        pu.points(Points::new(ds.name.clone(), PlotPoints::from(pts)).color(col).radius(3.0));
                    } else {
                        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
                        pu.line(Line::new(ds.name.clone(), PlotPoints::from(pts)).color(col).width(2.0));
                    }
                }
            });
            return;
        }
        // Image with all datasets of the active panel.
        let painter = ui.painter_at(view.rect);
        for d in self.datasets() {
            let col = c32(d.color);
            for p in &d.points {
                let s = view.to_screen(pos(p.pt()));
                painter.circle_filled(s, 3.0, col);
                painter.circle_stroke(s, 3.0, Stroke::new(0.5, Color32::BLACK));
            }
        }
    }
}
