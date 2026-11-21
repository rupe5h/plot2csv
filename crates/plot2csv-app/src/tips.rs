//! One-time guidance per stage: what the user is supposed to do there.
//!
//! A stage's tip appears the first time the stage is opened and is not shown
//! again in the same session. It counts as seen once dismissed or once the
//! user moves to another stage. The seen set is kept in the browser's
//! `sessionStorage` so it survives a page reload but not a new tab.

use egui::{Align2, RichText, Vec2};
use plot2csv_core::project::Stage;

use crate::icons::{self, Icon};
use crate::{platform, widgets};

const STORAGE_KEY: &str = "plot2csv.tipsSeen";
const CARD_WIDTH: f32 = 330.0;

struct StageTip {
    goal: &'static str,
    /// `{icon-name}` tokens are drawn as icons (see [`icons::rich_label`]).
    steps: &'static [&'static str],
}

fn tip(stage: Stage) -> StageTip {
    match stage {
        Stage::Import => StageTip {
            goal: "Bring in the plot you want to turn into numbers.",
            steps: &[
                "Load an image with {folder-open} Load Image…, drop it on the window, or paste it with Ctrl+V.",
                "For a phone photo of a printed plot, tick “Enhance photo” to even out the lighting.",
                "To resume earlier work, use Import Project… with a saved .plot2csv.json file.",
            ],
        },
        Stage::Correct => StageTip {
            goal: "Straighten the image so the plot's axes run horizontally and vertically.",
            steps: &[
                "The blue note on the right says what was detected. If the image is already straight, there is nothing to do.",
                "Otherwise, drag the blue frame's TL / TR / BR / BL corners onto the plot frame, or use the Rotate slider or {rotate-right} Rotate 90°.",
                "Check the result with {eye} Preview result.",
            ],
        },
        Stage::Region => StageTip {
            goal: "Mark where each plot sits in the image.",
            steps: &[
                "Blue box: the data area inside the axes. Grey dashed box: the same area plus tick labels and titles.",
                "Drag the box edges to fit. Use {plus} Draw a new plot area for extra panels.",
                "Set each panel's Type, and untick any panel you don't want to digitize.",
            ],
        },
        Stage::Text => StageTip {
            goal: "Check the text read from around the plot. The tick labels set the axis calibration.",
            steps: &[
                "Each box on the image is text found by OCR. Click a box to select it.",
                "Fix misread text and set its Role in the list. Make sure the tick values are right.",
                "Missed a label? Press {plus} Read text in a box and drag around it.",
            ],
        },
        Stage::Axes => StageTip {
            goal: "Link pixel positions to data values.",
            steps: &[
                "Calibration points are proposed from the tick labels. Check that each one sits exactly on its tick.",
                "Drag a point to move it, or select it and nudge it with the arrow keys (Shift ×10, Alt ×¼).",
                "Correct the axis values below the points. Tick Log scale for logarithmic axes.",
                "Hover over the image. The readout under the magnifier should show the right data values.",
            ],
        },
        Stage::Data => StageTip {
            goal: "Extract the data series from the plot.",
            steps: &[
                "Automatic Mode: pick the series colour with {eye-dropper} Pick and choose an Algorithm. Use Mask to limit where it looks; painting over a curve with the Pen also takes its colour.",
                "Manual Mode: click on the curve to add points. Keys: A add, S adjust, D delete.",
                "Add a dataset for each series with {plus} Add dataset. Check the values with View Data.",
            ],
        },
        Stage::Export => StageTip {
            goal: "Download the extracted data.",
            steps: &[
                "{download} Download all (ZIP) gives one CSV per series, combined CSVs and the project file.",
                "Long CSV and Wide CSV each download a single combined table.",
                "Tick “Show re-plotted data” to compare the result with the original image.",
                "Project (JSON) saves everything so you can come back to it later.",
            ],
        },
    }
}

/// Stable name used in storage (independent of enum order or titles).
fn key(stage: Stage) -> &'static str {
    match stage {
        Stage::Import => "import",
        Stage::Correct => "correct",
        Stage::Region => "region",
        Stage::Text => "text",
        Stage::Axes => "axes",
        Stage::Data => "data",
        Stage::Export => "export",
    }
}

pub struct Tips {
    seen: [bool; Stage::ALL.len()],
    /// The stage whose tip is currently open.
    open: Option<Stage>,
}

impl Tips {
    /// Restore which tips were already seen in this session.
    pub fn load() -> Self {
        let mut seen = [false; Stage::ALL.len()];
        if let Some(stored) = platform::session_get(STORAGE_KEY) {
            for name in stored.split(',') {
                if let Some(s) = Stage::ALL.iter().find(|s| key(**s) == name) {
                    seen[s.index()] = true;
                }
            }
        }
        Self { seen, open: None }
    }

    fn mark_seen(&mut self, stage: Stage) {
        if std::mem::replace(&mut self.seen[stage.index()], true) {
            return;
        }
        let names: Vec<&str> = Stage::ALL.iter().filter(|s| self.seen[s.index()]).map(|s| key(*s)).collect();
        platform::session_set(STORAGE_KEY, &names.join(","));
    }

    /// Open the tip for `stage` again on request (Help menu).
    pub fn reopen(&mut self, stage: Stage) {
        self.open = Some(stage);
    }

    /// Draw the tip for `stage` in the top-right corner of `area` if it has
    /// not been seen yet. `hold` keeps it hidden for now (busy or a dialog is
    /// open) without using it up.
    pub fn show(&mut self, ctx: &egui::Context, stage: Stage, area: egui::Rect, hold: bool) {
        // Leaving a stage while its tip is open counts as having seen it.
        if let Some(s) = self.open.filter(|s| *s != stage) {
            self.mark_seen(s);
            self.open = None;
        }
        if hold {
            return;
        }
        if self.open.is_none() && !self.seen[stage.index()] {
            self.open = Some(stage);
        }
        let Some(s) = self.open else { return };
        let t = tip(s);
        let mut dismiss = false;
        egui::Area::new(egui::Id::unique("stage_tip"))
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(area.right_top() + Vec2::new(-12.0, 12.0))
            .constrain_to(area)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(12.0).show(ui, |ui| {
                    ui.set_width(CARD_WIDTH);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("TIP").small().strong().color(widgets::INFO));
                        ui.label(RichText::new(format!("{} {}", widgets::STAGE_NUMS[s.index()], s.title())).strong().size(16.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            dismiss |= ui.small_button(icons::image(ui, Icon::Xmark)).on_hover_text("Close this tip").clicked();
                        });
                    });
                    ui.add_space(2.0);
                    ui.label(RichText::new(t.goal).strong());
                    ui.add_space(2.0);
                    for step in t.steps {
                        ui.horizontal_top(|ui| {
                            ui.label("•");
                            icons::rich_label(ui, step, egui::TextStyle::Body, ui.visuals().text_color());
                        });
                    }
                    if s != Stage::Export {
                        ui.add_space(2.0);
                        let weak = ui.visuals().weak_text_color();
                        icons::rich_label(
                            ui,
                            "When it looks right, press Approve & continue {chevron-right} (or Enter).",
                            egui::TextStyle::Body,
                            weak,
                        );
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        dismiss |= widgets::primary_button(ui, "Got it", true).clicked();
                        ui.label(RichText::new("Shown once. Reopen from Help → Tip for this stage.").weak().small());
                    });
                });
            });
        if dismiss {
            self.mark_seen(s);
            self.open = None;
        }
    }
}
