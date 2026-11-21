//! Modal dialogs in the style of WebPlotDigitizer (title bar in the accent colour).

use egui::{Color32, RichText};
use plot2csv_core::axes::AxesKind;
use plot2csv_core::dataset::ColorMode;
use plot2csv_core::project::Stage;

use crate::app::Plot2CsvApp;
use crate::icons::{self, Icon};
use crate::stages::axes_values_ui;
use crate::widgets::{self, ACCENT};

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    PlotType,
    CalibrationValues,
    ColorPicker { background: bool },
    Datasets,
    AcquiredData,
    Shortcuts,
    Licenses,
    Equations,
    About,
}

/// Show a modal with a title bar; returns (keep_open, inner).
fn modal<R>(ctx: &egui::Context, id: &str, title: &str, width: f32, body: impl FnOnce(&mut egui::Ui) -> R) -> (bool, R) {
    let resp = egui::Modal::new(egui::Id::unique(id)).show(ctx, |ui| {
        ui.set_width(width);
        let r = egui::Frame::new().fill(ACCENT).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong().color(Color32::BLACK));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.button(icons::image_tinted(ui, Icon::Xmark, Color32::BLACK)).on_hover_text("Close").clicked()
                })
                .inner
            })
            .inner
        });
        ui.add_space(6.0);
        (r.inner, body(ui))
    });
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let (closed, inner) = resp.inner;
    (!(closed || esc), inner)
}

const PLOT2CSV_LICENSE: &str = include_str!("../../../LICENSE");
/// Copy of indra-ocr's LICENSE (same SHA-256 as pinned in xtask).
const INDRA_MODELS_LICENSE: &str = include_str!("../licenses/indra-ocr-models.txt");
const ONNXRUNTIME_LICENSE: &str = include_str!("../licenses/onnxruntime.txt");
const INTER_LICENSE: &str = include_str!("../licenses/inter-ofl.txt");
const HACK_LICENSE: &str = include_str!("../licenses/hack.txt");
const FONTAWESOME_LICENSE: &str = include_str!("../licenses/fontawesome-free.txt");

fn license_text(ui: &mut egui::Ui, id: &str, title: &str, text: &str) {
    egui::CollapsingHeader::new(title).id_salt(id).default_open(false).show(ui, |ui| {
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(6).show(ui, |ui| {
            ui.label(RichText::new(text).small());
        });
    });
}

/// Body of Help → Licences: plot2csv (AGPL-3.0-or-later) and the bundled
/// components, with the OCR models' restrictive licence called out.
fn licenses_ui(ui: &mut egui::Ui) {
    ui.label(RichText::new("plot2csv").heading());
    ui.label("Copyright © 2026 the plot2csv authors.");
    ui.label("plot2csv is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License, version 3 or (at your option) any later version. If you distribute it, or let people use a modified version over a network, you must offer them its complete source code under the same licence. It comes with NO WARRANTY.");
    license_text(ui, "lic_agpl", "GNU Affero General Public License v3 (plot2csv)", PLOT2CSV_LICENSE);
    ui.add_space(8.0);

    ui.label(RichText::new("Bundled OCR models: a different licence").heading().color(widgets::WARN));
    egui::Frame::new().stroke(egui::Stroke::new(1.0, widgets::WARN)).corner_radius(4).inner_margin(8).show(ui, |ui| {
        ui.label(
            RichText::new("The AGPL above does NOT cover the OCR models.").strong(),
        );
        ui.label("Text recognition uses the indra-ocr detection and recognition models (v1.1.0) by Datalab, which are bundled with this app. Their weights are under Datalab's modified AI Pubs Open RAIL-M License, which restricts use:");
        for item in [
            "Free for personal use and research.",
            "Any other use is allowed only if you (or your employer, or the entity you are affiliated with) had no more than US$20 million in gross revenue in the prior year and have raised no more than US$20 million in total funding.",
            "Not for use by anyone who offers a product or service that competes with Datalab's.",
            "No use that breaks the law or infringes third-party intellectual property.",
            "Anyone who redistributes the models must pass on this licence and its use restrictions. The licence terms also extend to the models' output.",
        ] {
            ui.horizontal(|ui| {
                ui.label("•");
                ui.add(egui::Label::new(item).wrap());
            });
        }
        ui.horizontal(|ui| {
            ui.label("Commercial licences:");
            ui.hyperlink_to("datalab.to/pricing", "https://www.datalab.to/pricing");
        });
        ui.horizontal(|ui| {
            ui.label("Models:");
            ui.hyperlink_to("huggingface.co/datalab-to/indra-ocr", "https://huggingface.co/datalab-to/indra-ocr");
        });
        ui.label(RichText::new("This is a summary; the full licence below is what applies. Read it before using the OCR commercially or redistributing this app.").weak().small());
        license_text(ui, "lic_indra", "Full licence: indra-ocr models (modified Open RAIL-M)", INDRA_MODELS_LICENSE);
    });
    ui.add_space(8.0);

    ui.label(RichText::new("Other components").heading());
    ui.horizontal_wrapped(|ui| {
        ui.label("The OCR pre- and post-processing is a Rust port of Datalab's indra code, which is under the Apache License 2.0:");
        ui.hyperlink_to("github.com/datalab-to/indra", "https://github.com/datalab-to/indra");
    });
    ui.label("onnxruntime-web 1.31.1, which runs the OCR models in the browser, is under the MIT License, © Microsoft Corporation.");
    license_text(ui, "lic_ort", "MIT License (onnxruntime-web)", ONNXRUNTIME_LICENSE);
    ui.horizontal_wrapped(|ui| {
        ui.label("The interface font is Inter, © The Inter Project Authors, under the SIL Open Font License 1.1:");
        ui.hyperlink_to("rsms.me/inter", "https://rsms.me/inter/");
    });
    license_text(ui, "lic_inter", "SIL Open Font License 1.1 (Inter)", INTER_LICENSE);
    ui.label("The monospace font is Hack, © Source Foundry Authors, under the MIT License, with parts under the Bitstream Vera License.");
    license_text(ui, "lic_hack", "Hack licence", HACK_LICENSE);
    ui.horizontal_wrapped(|ui| {
        ui.label("Icons are from Font Awesome Free 7.3.1, © Fonticons, Inc., under the CC BY 4.0 licence:");
        ui.hyperlink_to("fontawesome.com", "https://fontawesome.com");
    });
    license_text(ui, "lic_fa", "Font Awesome Free licence", FONTAWESOME_LICENSE);
    ui.label("The Rust libraries compiled into the app (egui, eframe, image, imageproc, nalgebra and others) are under the MIT and/or Apache-2.0 licences.");
}

impl Plot2CsvApp {
    pub fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialog.clone() else { return };
        let keep = match d {
            Dialog::PlotType => self.plot_type_dialog(ctx),
            Dialog::CalibrationValues => self.cal_values_dialog(ctx),
            Dialog::ColorPicker { background } => self.color_dialog(ctx, background),
            Dialog::Datasets => self.datasets_dialog(ctx),
            Dialog::AcquiredData => {
                let (open, close) = modal(ctx, "acq", "Acquired Data", 520.0, |ui| {
                    self.acquired_data_ui(ui);
                    ui.add_space(4.0);
                    ui.button("Close").clicked()
                });
                open && !close
            }
            Dialog::Shortcuts => {
                let (open, close) = modal(ctx, "keys", "Keyboard Shortcuts", 700.0, |ui| {
                    egui::Grid::new("keys").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                        ui.label(RichText::new("Action").strong());
                        ui.label(RichText::new("Windows / Linux").strong());
                        ui.label(RichText::new("macOS").strong());
                        ui.end_row();
                        // (action, Windows / Linux, macOS). Key names are spelled out:
                        // the bundled fonts have no ⌘ or ⌥ glyphs.
                        for (action, pc, mac) in [
                            ("Zoom about the pointer", "Mouse wheel / pinch", "Two-finger scroll / pinch"),
                            ("Pan", "Space+drag, right- or middle-drag", "Space+drag, right-drag"),
                            ("Zoom in / out / fit", "+ / − / F", "+ / − / F"),
                            ("Approve the current stage and continue", "Enter", "Return"),
                            ("Undo", "Ctrl+Z", "Cmd+Z"),
                            ("Redo", "Ctrl+Y or Ctrl+Shift+Z", "Cmd+Shift+Z"),
                            ("Paste an image", "Ctrl+V", "Cmd+V"),
                            ("Add / Adjust / Delete point (Data stage)", "A / S / D", "A / S / D"),
                            ("Move the selected point by 1 px", "Arrow keys", "Arrow keys"),
                            ("Move by 10 px", "Shift+Arrow", "Shift+Arrow"),
                            ("Move by ¼ px", "Alt+Arrow", "Option+Arrow"),
                            ("Select next / previous point", "Q / W", "Q / W"),
                            ("Delete the selected point or text box", "Delete or Backspace", "Delete"),
                            ("Close a dialog", "Esc", "Esc"),
                        ] {
                            ui.label(action);
                            ui.label(RichText::new(pc).strong());
                            ui.label(RichText::new(mac).strong());
                            ui.end_row();
                        }
                    });
                    ui.add_space(4.0);
                    ui.button("Close").clicked()
                });
                open && !close
            }
            Dialog::Licenses => {
                let max_h = ctx.content_rect().height() * 0.7;
                let (open, close) = modal(ctx, "licenses", "Licences", 560.0, |ui| {
                    egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, licenses_ui);
                    ui.add_space(4.0);
                    ui.button("Close").clicked()
                });
                open && !close
            }
            Dialog::Equations => {
                let text = match self.active_calibration() {
                    Some(plot2csv_core::Calibration::Xy(c)) => c.equations(),
                    Some(_) => "Equations are shown for X-Y calibrations.".into(),
                    None => "The axes are not calibrated yet.".into(),
                };
                let (open, close) = modal(ctx, "eq", "Transformation Equations", 460.0, |ui| {
                    ui.label(RichText::new(text).monospace());
                    ui.button("Close").clicked()
                });
                open && !close
            }
            Dialog::About => {
                let (open, close) = modal(ctx, "about", "About plot2csv", 420.0, |ui| {
                    ui.label(RichText::new("plot2csv — plot digitizer").heading());
                    ui.label("Extract numerical data from images of plots, in the spirit of WebPlotDigitizer, with an automatic pipeline you review at every step: rotation/perspective correction, plot-area detection, OCR of labels, axis calibration and series extraction.");
                    ui.add_space(4.0);
                    ui.label("OCR: indra-ocr models (Datalab) on onnxruntime-web.");
                    ui.label("Runs entirely in your browser; nothing is uploaded.");
                    ui.add_space(4.0);
                    ui.label("plot2csv is licensed under the GNU AGPL v3 or later. The bundled OCR models are under a different, more restrictive licence: see Help → Licences.");
                    ui.button("Close").clicked()
                });
                open && !close
            }
        };
        if !keep && self.dialog.as_ref() == Some(&d) {
            self.dialog = None;
        }
    }

    fn plot_type_dialog(&mut self, ctx: &egui::Context) -> bool {
        let cur = self.project.active_panel.and_then(|id| self.project.panel(id)).map(|p| p.kind).unwrap_or_default();
        let id = egui::Id::unique("plot_type_choice");
        let mut choice: AxesKind = ctx.data(|d| d.get_temp(id)).unwrap_or(cur);
        let (open, (align, cancel)) = modal(ctx, "ptype", "Choose Plot Type", 300.0, |ui| {
            for k in AxesKind::ALL {
                ui.radio_value(&mut choice, k, k.label());
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| (ui.button("Align Axes").clicked(), ui.button("Cancel").clicked())).inner
        });
        ctx.data_mut(|d| d.insert_temp(id, choice));
        if align {
            ctx.data_mut(|d| d.remove::<AxesKind>(id));
            if choice != cur || self.stage != Stage::Axes {
                self.set_plot_kind(ctx, choice);
            }
            if self.project.can_open(Stage::Axes) {
                self.goto(ctx, Stage::Axes);
            }
            return false;
        }
        if cancel || !open {
            ctx.data_mut(|d| d.remove::<AxesKind>(id));
            return false;
        }
        true
    }

    fn cal_values_dialog(&mut self, ctx: &egui::Context) -> bool {
        let Some(mut axes) = self.axes_mut().cloned() else { return false };
        let title = match axes.kind {
            AxesKind::Xy => "X and Y Axes Calibration",
            AxesKind::Polar => "Align Polar Axes",
            AxesKind::Ternary => "Select Range of Variables",
            AxesKind::Map => "Scale Size",
            AxesKind::Bar => "Bar Chart Calibration",
            AxesKind::Image => "Image Axes",
        };
        let (open, (changed, ok)) = modal(ctx, "calvals", title, 380.0, |ui| {
            if axes.kind == AxesKind::Xy {
                ui.label("Enter X-values of the two points clicked on the X-axis and Y-values of the two points clicked on the Y-axis.");
            }
            let c = axes_values_ui(ui, &mut axes);
            if let Err(e) = axes.calibrate() {
                ui.label(RichText::new(e).color(widgets::WARN).small());
            }
            ui.add_space(4.0);
            (c, ui.button("OK").clicked())
        });
        if changed {
            if let Some(a) = self.axes_mut() {
                *a = axes;
            }
            self.edited(Stage::Axes);
        }
        open && !ok
    }

    fn color_dialog(&mut self, ctx: &egui::Context, background: bool) -> bool {
        let Some(ds) = self.datasets().get(self.active_ds_index()).cloned() else { return false };
        let mut c = if background { ds.auto.bg } else { ds.auto.fg };
        let dom: Vec<[u8; 3]> = match &self.imgs.corrected {
            Some(img) => {
                let inner = self.project.active_panel.and_then(|id| self.project.panel(id)).map(|p| p.inner);
                plot2csv_vision::color::dominant_colors(img, inner, 10, 12.0).into_iter().map(|c| c.color).collect()
            }
            None => vec![],
        };
        let title = if background { "Specify Plot (Background) Color" } else { "Specify Plot (Foreground) Color" };
        let (open, done) = modal(ctx, "colordlg", title, 330.0, |ui| {
            ui.vertical_centered(|ui| {
                widgets::swatch(ui, c, 36.0);
            });
            ui.horizontal(|ui| {
                ui.label("R:");
                ui.add(egui::DragValue::new(&mut c[0]));
                ui.label("G:");
                ui.add(egui::DragValue::new(&mut c[1]));
                ui.label("B:");
                ui.add(egui::DragValue::new(&mut c[2]));
                egui::color_picker::color_edit_button_srgb(ui, &mut c);
            });
            ui.label("Dominant Colors:");
            ui.horizontal_wrapped(|ui| {
                for d in &dom {
                    if widgets::swatch(ui, *d, 22.0).clicked() {
                        c = *d;
                    }
                }
            });
            let weak = ui.visuals().weak_text_color();
            icons::rich_label(
                ui,
                "Tip: use “{eye-dropper} Pick” in the sidebar to take the colour from the image.",
                egui::TextStyle::Small,
                weak,
            );
            ui.button("Done").clicked()
        });
        let old = if background { ds.auto.bg } else { ds.auto.fg };
        if c != old {
            if let Some(d) = self.active_ds_mut() {
                if background {
                    d.auto.bg = c;
                    d.auto.color_mode = ColorMode::Background;
                } else {
                    d.auto.fg = c;
                    d.color = c;
                }
            }
            self.data_ui.invalidate_caches();
            self.edited(Stage::Data);
        }
        open && !done
    }

    fn datasets_dialog(&mut self, ctx: &egui::Context) -> bool {
        let dss = self.datasets();
        let active = self.active_ds_index();
        let name_id = egui::Id::unique("ds_name_edit");
        let mut name: String =
            ctx.data(|d| d.get_temp(name_id)).unwrap_or_else(|| dss.get(active).map(|d| d.name.clone()).unwrap_or_default());
        let mut select = None;
        let mut change = false;
        let (open, (enter, add, del, view, close)) = modal(ctx, "dsmgr", "Manage Datasets", 360.0, |ui| {
            egui::Grid::new("dsm").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Selected Dataset:");
                egui::ComboBox::from_id_salt("dsm_sel").selected_text(dss.get(active).map(|d| d.name.clone()).unwrap_or_default()).show_ui(
                    ui,
                    |ui| {
                        for (i, d) in dss.iter().enumerate() {
                            if ui.selectable_label(i == active, &d.name).clicked() {
                                select = Some(i);
                            }
                        }
                    },
                );
                ui.end_row();
                ui.label("Dataset Name:");
                change = ui
                    .horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
                        ui.button("Change").clicked()
                    })
                    .inner;
                ui.end_row();
                ui.label("Data Points:");
                ui.label(format!("{}", dss.get(active).map(|d| d.points.len()).unwrap_or(0)));
                ui.end_row();
            });
            ui.add_space(6.0);
            let r = ui
                .horizontal(|ui| {
                    (
                        ui.button("Add").clicked(),
                        ui.button("Delete").clicked(),
                        ui.button("View Data").clicked(),
                        ui.button("Close").clicked(),
                    )
                })
                .inner;
            (ui.input(|i| i.key_pressed(egui::Key::Enter)), r.0, r.1, r.2, r.3)
        });
        let change = change || enter;
        ctx.data_mut(|d| d.insert_temp(name_id, name.clone()));
        if let Some(i) = select {
            self.set_active_ds(i);
            ctx.data_mut(|d| d.remove::<String>(name_id));
        }
        if change && !name.trim().is_empty() {
            if let Some(d) = self.active_ds_mut() {
                d.name = name.trim().to_string();
            }
            self.edited(Stage::Data);
        }
        if add {
            self.add_dataset(None);
            ctx.data_mut(|d| d.remove::<String>(name_id));
        }
        if del && !dss.is_empty() {
            if let Some(v) = self.datasets_mut() {
                if active < v.len() {
                    v.remove(active);
                }
            }
            self.set_active_ds(active.saturating_sub(1));
            self.edited(Stage::Data);
            ctx.data_mut(|d| d.remove::<String>(name_id));
        }
        if view {
            ctx.data_mut(|d| d.remove::<String>(name_id));
            self.dialog = Some(Dialog::AcquiredData);
            return true;
        }
        if close || !open {
            ctx.data_mut(|d| d.remove::<String>(name_id));
            return false;
        }
        true
    }
}
