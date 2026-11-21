//! ⑦ Export: WebPlotDigitizer's "Acquired Data" view, CSV/ZIP/JSON export and a plot preview.

use egui::{Color32, Stroke};
use egui_plot::{HoverPosition, Line, Plot, PlotPoints, Points};
use plot2csv_core::axes::Calibration;
use plot2csv_core::dataset::Dataset;
use plot2csv_core::export::{export_zip, long_csv, sanitize_filename, series_csv, wide_csv, SeriesRef};
use plot2csv_core::table::{format_rows, headers, rows, AxisTitles, SortBy, TableOptions};
use plot2csv_core::NumFormat;

use super::pos;
use crate::app::Plot2CsvApp;
use crate::canvas::CanvasView;
use crate::icons::Icon;
use crate::log_axis;
use crate::platform;
use crate::widgets::{self, c32};

#[derive(Default)]
pub struct ExportUi {
    /// Panel id and dataset index shown in the Acquired Data table; `None`: all series of the panel.
    pub sel: Option<(u32, Option<usize>)>,
    pub plot_preview: bool,
}

/// A dataset with what its export needs: panel, calibration and the axis titles.
#[derive(Clone)]
pub struct SeriesOwned {
    pub panel_id: u32,
    pub panel: String,
    pub cal: Calibration,
    pub ds: Dataset,
    pub titles: AxisTitles,
}

impl SeriesOwned {
    pub fn as_ref(&self) -> SeriesRef<'_> {
        SeriesRef { panel: &self.panel, cal: &self.cal, ds: &self.ds, titles: &self.titles }
    }
}

/// Bar charts are always exported with every series as a column.
fn all_columns_only(cal: &Calibration) -> bool {
    matches!(cal, Calibration::Bar(_))
}

impl Plot2CsvApp {
    /// Every dataset of the enabled panels, with a usable calibration (pixels if none).
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
            let titles = AxisTitles::from_items(w.text.value.as_deref().unwrap_or_default());
            for d in w.data.value.iter().flatten() {
                out.push(SeriesOwned { panel_id: p.id, panel: p.name.clone(), cal: cal.clone(), ds: d.clone(), titles: titles.clone() });
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
        let refs: Vec<SeriesRef> = all.iter().map(SeriesOwned::as_ref).collect();
        let json = self.project.to_json();
        match export_zip(&refs, &self.table_opts, Some(&json)) {
            Ok(z) => {
                platform::save_file(&format!("{}_data.zip", self.base_name()), z);
                self.status = format!("Exported {} series.", refs.len());
            }
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    /// All series of one panel side by side (X-Y, bars), or in long format (other plot types).
    fn all_series_table(series: &[SeriesRef], opts: &TableOptions) -> String {
        wide_csv(series, opts).unwrap_or_else(|| long_csv(series, opts))
    }

    /// The Acquired Data table (used by the Export stage and the View Data dialog).
    pub fn acquired_data_ui(&mut self, ui: &mut egui::Ui) {
        let all = self.all_series();
        if all.is_empty() {
            ui.label("No data yet.");
            return;
        }
        let mut panels: Vec<(u32, String, bool)> = Vec::new();
        for s in &all {
            if !panels.iter().any(|p| p.0 == s.panel_id) {
                panels.push((s.panel_id, s.panel.clone(), all_columns_only(&s.cal)));
            }
        }
        let multi_panel = panels.len() > 1;
        // Default selection: the active panel's active dataset (bar charts: all series).
        let active = self.project.active_panel.filter(|id| panels.iter().any(|p| p.0 == *id)).unwrap_or(panels[0].0);
        let mut sel = self.export_ui.sel.unwrap_or((active, Some(self.active_ds_index())));
        let Some(&(_, _, bars)) = panels.iter().find(|p| p.0 == sel.0).or(panels.first()) else { return };
        if !panels.iter().any(|p| p.0 == sel.0) {
            sel = (panels[0].0, Some(0));
        }
        let in_panel: Vec<&SeriesOwned> = all.iter().filter(|s| s.panel_id == sel.0).collect();
        sel.1 = if bars { None } else { sel.1.map(|i| i.min(in_panel.len().saturating_sub(1))) };
        let panel_prefix = |name: &str| if multi_panel { format!("{name} / ") } else { String::new() };
        ui.horizontal(|ui| {
            ui.label("Dataset:");
            let cur = match sel.1 {
                None => format!("{}All series", panel_prefix(&in_panel[0].panel)),
                Some(i) => in_panel.get(i).map(|s| format!("{}{}", panel_prefix(&s.panel), s.ds.name)).unwrap_or_default(),
            };
            egui::ComboBox::from_id_salt("acq_ds").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                for (pid, pname, bars) in &panels {
                    let n = all.iter().filter(|s| s.panel_id == *pid).count();
                    let all_label = format!("{}All series ({n})", panel_prefix(pname));
                    if ui.selectable_label(sel == (*pid, None), all_label).on_hover_text("Every series of the panel as a column").clicked()
                    {
                        sel = (*pid, None);
                    }
                    if *bars {
                        continue;
                    }
                    for (k, s) in all.iter().filter(|s| s.panel_id == *pid).enumerate() {
                        if ui.selectable_label(sel == (*pid, Some(k)), format!("{}{}", panel_prefix(pname), s.ds.name)).clicked() {
                            sel = (*pid, Some(k));
                        }
                    }
                }
            });
        });
        self.export_ui.sel = Some(sel);
        let in_panel: Vec<SeriesOwned> = all.iter().filter(|s| s.panel_id == sel.0).cloned().collect();
        let Some(first) = in_panel.first().cloned() else { return };
        let one = sel.1.and_then(|i| in_panel.get(i).cloned());
        let cal = first.cal.clone();
        let vars = headers(&cal, &first.titles);
        match (&one, &cal) {
            (Some(_), _) => ui.label(format!("Columns: {}", vars.join(", "))),
            (None, Calibration::Xy(_) | Calibration::Bar(_)) => ui.label(format!("Columns: {}, then one per series", vars[0])),
            (None, _) => ui.label(format!("Columns: panel, series, {}", vars.join(", "))),
        };
        if one.is_none() {
            let how = match cal {
                Calibration::Bar(_) => "Bar chart: one row per category, one column per series.",
                Calibration::Xy(_) => "One row for every X value of any series, so no point is left out; each series is interpolated linearly between its own points at the others' X values (empty outside its range).",
                _ => "Every series in turn: panel, series, values.",
            };
            ui.label(egui::RichText::new(how).weak().small());
        }
        let opts = &mut self.table_opts;
        egui::Grid::new("acq_opts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            if one.is_some() {
                ui.label(egui::RichText::new("Sort by").strong());
                ui.horizontal(|ui| {
                    let col_name = |c: usize| vars.get(c + usize::from(matches!(cal, Calibration::Bar(_)))).cloned().unwrap_or_default();
                    let label = match opts.sort_by {
                        SortBy::Raw => "Raw".to_string(),
                        SortBy::Column(c) => col_name(c),
                        SortBy::NearestNeighbor => "Nearest Neighbor".into(),
                    };
                    egui::ComboBox::from_id_salt("sortby").selected_text(label).show_ui(ui, |ui| {
                        ui.selectable_value(&mut opts.sort_by, SortBy::Raw, "Raw");
                        let ncols = cal.to_data(plot2csv_core::geom::Pt::default()).len();
                        for c in 0..ncols {
                            ui.selectable_value(&mut opts.sort_by, SortBy::Column(c), col_name(c));
                        }
                        ui.selectable_value(&mut opts.sort_by, SortBy::NearestNeighbor, "Nearest Neighbor");
                    });
                    ui.add_enabled_ui(matches!(opts.sort_by, SortBy::Column(_)), |ui| {
                        egui::ComboBox::from_id_salt("order")
                            .selected_text(if opts.ascending { "Ascending" } else { "Descending" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut opts.ascending, true, "Ascending");
                                ui.selectable_value(&mut opts.ascending, false, "Descending");
                            });
                    });
                });
                ui.end_row();
            }
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
                ui.checkbox(&mut opts.header, "Header row").on_hover_text("Axis titles (and series names) as column headers");
            });
            ui.end_row();
        });
        let refs: Vec<SeriesRef> = in_panel.iter().map(SeriesOwned::as_ref).collect();
        let mut text = match &one {
            Some(s) => format_rows(&s.cal, &rows(&s.cal, &s.ds, &self.table_opts), &self.table_opts, &s.titles),
            None => Self::all_series_table(&refs, &self.table_opts),
        };
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
            if ui.button("Download .CSV").on_hover_text("With the header row").clicked() {
                let base = sanitize_filename(&self.base_name());
                let (name, csv) = match &one {
                    Some(s) => (format!("{base}_{}.csv", sanitize_filename(&s.ds.name)), series_csv(&s.as_ref(), &self.table_opts)),
                    None => {
                        let suffix =
                            if multi_panel { format!("{}_all_series", sanitize_filename(&first.panel)) } else { "all_series".into() };
                        let o = TableOptions { header: true, ..self.table_opts.clone() };
                        (format!("{base}_{suffix}.csv"), Self::all_series_table(&refs, &o))
                    }
                };
                platform::save_file(&name, csv.into_bytes());
            }
        });
    }

    pub fn export_sidebar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        widgets::section(ui, "⑦ Export all series");
        let all = self.all_series();
        ui.label(
            egui::RichText::new(format!(
                "{} series in {} panel(s).",
                all.len(),
                self.project.panel_list().iter().filter(|p| p.enabled).count()
            ))
            .weak(),
        );
        ui.horizontal_wrapped(|ui| {
            if widgets::primary_button_icon(ui, Icon::Download, "Download all (ZIP)", !all.is_empty()).on_hover_text("One CSV per series, a combined long CSV, wide CSVs and the project").clicked() {
                self.export_zip();
            }
            if ui.add_enabled(!all.is_empty(), egui::Button::new("Long CSV")).on_hover_text("panel, series, values").clicked() {
                let refs: Vec<SeriesRef> = all.iter().map(SeriesOwned::as_ref).collect();
                platform::save_file(&format!("{}_all_series_long.csv", self.base_name()), long_csv(&refs, &self.table_opts).into_bytes());
            }
            let pid = self.project.active_panel;
            let panel_refs: Vec<SeriesRef> = all.iter().filter(|s| Some(s.panel_id) == pid).map(SeriesOwned::as_ref).collect();
            let wide = wide_csv(&panel_refs, &TableOptions { header: true, ..self.table_opts.clone() });
            if ui
                .add_enabled(wide.is_some(), egui::Button::new("Wide CSV"))
                .on_hover_text("One column per series: a shared X column with the series interpolated (X-Y plots), or one row per category (bar charts)")
                .clicked()
            {
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
            let series: Vec<&SeriesOwned> = all.iter().filter(|s| Some(s.panel_id) == pid).collect();
            // Datasets of a panel share its calibration, so its log axes apply to all of them.
            let (log_x, log_y) = series.first().map(|s| preview_log_axes(&s.cal)).unwrap_or_default();
            let rect = view.rect;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            child.painter().rect_filled(rect, 0.0, child.visuals().panel_fill);
            // A separate id per scale so a remembered (panned) view isn't reused in the other space.
            let mut plot = Plot::new(("preview", log_x, log_y)).legend(egui_plot::Legend::default());
            if let Some(s) = series.first().filter(|s| matches!(s.cal, Calibration::Xy(_) | Calibration::Bar(_))) {
                let [x, y] = [&s.titles.x, &s.titles.y];
                if let (Some(x), false) = (x, matches!(s.cal, Calibration::Bar(_))) {
                    plot = plot.x_axis_label(x.clone());
                }
                if let Some(y) = y {
                    plot = plot.y_axis_label(y.clone());
                }
            }
            if log_x {
                plot = plot.x_grid_spacer(log_axis::grid_marks).x_axis_formatter(log_axis::tick_label);
            }
            if log_y {
                plot = plot.y_grid_spacer(log_axis::grid_marks).y_axis_formatter(log_axis::tick_label);
            }
            if log_x || log_y {
                let fmt = move |v: f64, log: bool| if log { log_axis::format_value(10f64.powf(v), 4) } else { format!("{v:.3}") };
                plot = plot.label_formatter(move |pos| {
                    Some(match pos {
                        HoverPosition::NearDataPoint { plot_name, position, .. } => {
                            format!("{plot_name}\nx = {}\ny = {}", fmt(position.x, log_x), fmt(position.y, log_y))
                        }
                        HoverPosition::Elsewhere { position } => format!("x = {}\ny = {}", fmt(position.x, log_x), fmt(position.y, log_y)),
                    })
                });
            }
            let scale = |v: f64, log: bool| if log { v.log10() } else { v };
            plot.show(&mut child, |pu| {
                for SeriesOwned { cal, ds, .. } in series {
                    let mut pts: Vec<[f64; 2]> = ds
                        .points
                        .iter()
                        .map(|p| {
                            let [x, y] = preview_xy(cal, p.pt());
                            [scale(x, log_x), scale(y, log_y)]
                        })
                        .filter(|p| p[0].is_finite() && p[1].is_finite())
                        .collect();
                    let col = c32(ds.color);
                    if matches!(ds.style, plot2csv_core::SeriesStyle::Markers | plot2csv_core::SeriesStyle::Bars) {
                        pu.points(Points::new(ds.name.clone(), PlotPoints::from(pts)).color(col).radius(3.0));
                    } else {
                        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                        // No "band" shortcut: it draws dense stretches with a vertical
                        // instead of a perpendicular width (thin steep segments, wedges at spikes).
                        pu.line(Line::new(ds.name.clone(), PlotPoints::from(pts)).color(col).width(2.0).allow_band(false));
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

/// The (x, y) a data point is re-plotted at: the first two values, or for single-value
/// calibrations (bars) the pixel x against the value.
fn preview_xy(cal: &Calibration, p: plot2csv_core::geom::Pt) -> [f64; 2] {
    let d = cal.to_data(p);
    if d.len() >= 2 {
        [d[0], d[1]]
    } else {
        [p.x, d[0]]
    }
}

/// Which re-plotted axes (x, y) are logarithmic on the source plot, matching [`preview_xy`].
fn preview_log_axes(cal: &Calibration) -> (bool, bool) {
    match cal {
        Calibration::Xy(c) => (c.log_x, c.log_y),
        Calibration::Bar(c) => (false, c.log),
        Calibration::Polar(c) => (c.log_r, false),
        Calibration::Ternary(_) | Calibration::Map(_) | Calibration::Image => (false, false),
    }
}
