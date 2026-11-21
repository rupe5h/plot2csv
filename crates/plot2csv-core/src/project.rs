//! The project: source image, per-stage proposals/decisions and per-panel work.
//! Serialises to JSON (File → Export/Import project).

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::axes::{Axes, AxesKind};
use crate::dataset::Dataset;
use crate::geom::{Homography, Pt, RectF};

pub const PROJECT_VERSION: u32 = 1;

/// The pipeline stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum Stage {
    #[default]
    Import,
    Correct,
    Region,
    Text,
    Axes,
    Data,
    Export,
}

impl Stage {
    pub const ALL: [Stage; 7] = [Stage::Import, Stage::Correct, Stage::Region, Stage::Text, Stage::Axes, Stage::Data, Stage::Export];

    pub fn title(self) -> &'static str {
        match self {
            Stage::Import => "Import",
            Stage::Correct => "Correct",
            Stage::Region => "Plot area",
            Stage::Text => "Text",
            Stage::Axes => "Axes",
            Stage::Data => "Data",
            Stage::Export => "Export",
        }
    }

    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap()
    }

    pub fn next(self) -> Option<Stage> {
        Stage::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<Stage> {
        self.index().checked_sub(1).map(|i| Stage::ALL[i])
    }

    /// Stages whose state lives per panel.
    pub fn per_panel(self) -> bool {
        matches!(self, Stage::Text | Stage::Axes | Stage::Data)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StageStatus {
    /// Not run yet.
    #[default]
    Pending,
    /// An automatic (or default) proposal is waiting for review.
    Proposed,
    /// Reviewed and accepted by the user.
    Approved,
    /// An earlier stage changed after this one was produced.
    Stale,
}

/// Proposal + current (possibly edited) value of one stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageState<T> {
    pub status: StageStatus,
    /// The last automatic proposal (for "Reset to auto").
    pub auto: Option<T>,
    /// The current value, edited by the user.
    pub value: Option<T>,
    /// Short explanation of the automatic result (confidence etc.).
    #[serde(default)]
    pub note: String,
}

impl<T> Default for StageState<T> {
    fn default() -> Self {
        Self { status: StageStatus::Pending, auto: None, value: None, note: String::new() }
    }
}

impl<T: Clone> StageState<T> {
    pub fn propose(&mut self, v: T, note: impl Into<String>) {
        self.auto = Some(v.clone());
        self.value = Some(v);
        self.status = StageStatus::Proposed;
        self.note = note.into();
    }

    pub fn reset_to_auto(&mut self) {
        if let Some(a) = &self.auto {
            self.value = Some(a.clone());
            if self.status == StageStatus::Approved {
                self.status = StageStatus::Proposed;
            }
        }
    }

    pub fn approve(&mut self) {
        if self.value.is_some() {
            self.status = StageStatus::Approved;
        }
    }

    pub fn mark_stale(&mut self) {
        if matches!(self.status, StageStatus::Proposed | StageStatus::Approved) {
            self.status = StageStatus::Stale;
        }
    }

    pub fn is_approved(&self) -> bool {
        self.status == StageStatus::Approved
    }
}

/// Image correction: the quad (source px, order TL, TR, BR, BL) maps onto an
/// `out_w × out_h` rectangle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    pub enabled: bool,
    pub quad: [Pt; 4],
    pub out_w: u32,
    pub out_h: u32,
    /// How the proposal was made, e.g. "plot frame", "deskew", "none".
    pub method: String,
    pub confidence: f64,
}

impl Correction {
    /// No correction for an image of the given size.
    pub fn identity(w: u32, h: u32) -> Self {
        let (wf, hf) = (w as f64, h as f64);
        Self {
            enabled: false,
            quad: [Pt::new(0.0, 0.0), Pt::new(wf, 0.0), Pt::new(wf, hf), Pt::new(0.0, hf)],
            out_w: w,
            out_h: h,
            method: "none".into(),
            confidence: 1.0,
        }
    }

    /// Source px -> corrected px.
    pub fn homography(&self) -> Homography {
        let (w, h) = (self.out_w as f64, self.out_h as f64);
        let dst = [Pt::new(0.0, 0.0), Pt::new(w, 0.0), Pt::new(w, h), Pt::new(0.0, h)];
        Homography::fit(&self.quad, &dst).unwrap_or_default()
    }

    /// Output size implied by the quad's side lengths (keeps the scale of the source).
    pub fn natural_size(quad: &[Pt; 4]) -> (u32, u32) {
        let w = (quad[0].dist(quad[1]) + quad[3].dist(quad[2])) / 2.0;
        let h = (quad[0].dist(quad[3]) + quad[1].dist(quad[2])) / 2.0;
        (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
    }

    /// Rotate the output by 90° clockwise (cyclic shift of the quad corners).
    pub fn rotate90(&mut self) {
        self.quad = [self.quad[3], self.quad[0], self.quad[1], self.quad[2]];
        std::mem::swap(&mut self.out_w, &mut self.out_h);
        self.enabled = true;
    }
}

/// One plot inside the (corrected) image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub id: u32,
    pub name: String,
    pub kind: AxesKind,
    /// The data area bounded by the axes/frame.
    pub inner: RectF,
    /// Inner area plus tick labels and titles (used for OCR).
    pub outer: RectF,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TextRole {
    XTick,
    YTick,
    XTitle,
    YTitle,
    Legend,
    Title,
    #[default]
    Other,
}

impl TextRole {
    pub const ALL: [TextRole; 7] =
        [TextRole::XTick, TextRole::YTick, TextRole::XTitle, TextRole::YTitle, TextRole::Legend, TextRole::Title, TextRole::Other];
    pub fn label(self) -> &'static str {
        match self {
            TextRole::XTick => "X tick",
            TextRole::YTick => "Y tick",
            TextRole::XTitle => "X title",
            TextRole::YTitle => "Y title",
            TextRole::Legend => "Legend",
            TextRole::Title => "Title",
            TextRole::Other => "Ignore",
        }
    }
    pub fn color(self) -> [u8; 3] {
        match self {
            TextRole::XTick => [230, 159, 0],
            TextRole::YTick => [86, 180, 233],
            TextRole::XTitle => [213, 94, 0],
            TextRole::YTitle => [0, 114, 178],
            TextRole::Legend => [0, 158, 115],
            TextRole::Title => [204, 121, 167],
            TextRole::Other => [140, 140, 140],
        }
    }
}

/// A recognised (or user-entered) piece of text in corrected-image px.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextItem {
    pub bbox: RectF,
    pub text: String,
    pub conf: f32,
    pub role: TextRole,
    /// Parsed numeric value (ticks), or None.
    #[serde(default)]
    pub value: Option<f64>,
    /// True if the text was typed or edited by the user.
    #[serde(default)]
    pub manual: bool,
    /// Rotated text (vertical axis titles).
    #[serde(default)]
    pub vertical: bool,
}

/// Distance and angle measurements (Analyze menu).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Measurements {
    pub distances: Vec<[Pt; 2]>,
    pub angles: Vec<[Pt; 3]>,
}

/// Detected tick/grid positions kept for overlays and residuals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct AxisEvidence {
    /// Pixel x of ticks/grid lines along the X axis.
    pub x_ticks: Vec<f64>,
    /// Pixel y of ticks/grid lines along the Y axis.
    pub y_ticks: Vec<f64>,
    /// Label value matched to each pixel position: (px, value, text index).
    pub x_matches: Vec<(f64, f64, usize)>,
    pub y_matches: Vec<(f64, f64, usize)>,
    /// Axis lines in pixel coordinates.
    pub x_axis: Option<[Pt; 2]>,
    pub y_axis: Option<[Pt; 2]>,
}

/// Work done on one panel (stages Text, Axes, Data).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelWork {
    pub panel_id: u32,
    pub text: StageState<Vec<TextItem>>,
    pub axes: StageState<Axes>,
    #[serde(default)]
    pub evidence: AxisEvidence,
    pub data: StageState<Vec<Dataset>>,
    #[serde(default)]
    pub measurements: Measurements,
    #[serde(default)]
    pub active_dataset: usize,
}

impl PanelWork {
    pub fn new(panel_id: u32) -> Self {
        Self {
            panel_id,
            text: StageState::default(),
            axes: StageState::default(),
            evidence: AxisEvidence::default(),
            data: StageState::default(),
            measurements: Measurements::default(),
            active_dataset: 0,
        }
    }

    pub fn status(&self, stage: Stage) -> StageStatus {
        match stage {
            Stage::Text => self.text.status,
            Stage::Axes => self.axes.status,
            Stage::Data => self.data.status,
            _ => StageStatus::Pending,
        }
    }
}

/// The source image, stored as PNG/JPEG bytes (base64 in JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(with = "b64")]
    pub bytes: Vec<u8>,
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportOptions {
    /// Flatten uneven lighting and normalise white balance (phone photos).
    pub enhance: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub source: Option<SourceImage>,
    #[serde(default)]
    pub import: StageState<ImportOptions>,
    pub correction: StageState<Correction>,
    pub regions: StageState<Vec<Panel>>,
    pub panels: Vec<PanelWork>,
    pub active_panel: Option<u32>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: PROJECT_VERSION,
            source: None,
            import: StageState::default(),
            correction: StageState::default(),
            regions: StageState::default(),
            panels: Vec::new(),
            active_panel: None,
        }
    }
}

impl Project {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("project serialises")
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let p: Project = serde_json::from_str(s).map_err(|e| format!("not a plot2csv project: {e}"))?;
        if p.version > PROJECT_VERSION {
            return Err(format!("project version {} is newer than this app ({PROJECT_VERSION})", p.version));
        }
        Ok(p)
    }

    pub fn panel_list(&self) -> &[Panel] {
        self.regions.value.as_deref().unwrap_or(&[])
    }

    pub fn panel(&self, id: u32) -> Option<&Panel> {
        self.panel_list().iter().find(|p| p.id == id)
    }

    pub fn work(&self, id: u32) -> Option<&PanelWork> {
        self.panels.iter().find(|w| w.panel_id == id)
    }

    pub fn work_mut(&mut self, id: u32) -> &mut PanelWork {
        if let Some(i) = self.panels.iter().position(|w| w.panel_id == id) {
            &mut self.panels[i]
        } else {
            self.panels.push(PanelWork::new(id));
            self.panels.last_mut().unwrap()
        }
    }

    pub fn active_work(&self) -> Option<&PanelWork> {
        self.active_panel.and_then(|id| self.work(id))
    }

    /// Status of a stage (per-panel stages use the active panel).
    pub fn status(&self, stage: Stage) -> StageStatus {
        match stage {
            Stage::Import => {
                if self.source.is_some() {
                    self.import.status.max_with(StageStatus::Proposed)
                } else {
                    StageStatus::Pending
                }
            }
            Stage::Correct => self.correction.status,
            Stage::Region => self.regions.status,
            Stage::Text | Stage::Axes | Stage::Data => self.active_work().map(|w| w.status(stage)).unwrap_or(StageStatus::Pending),
            Stage::Export => {
                if self.active_work().map(|w| w.data.is_approved()).unwrap_or(false) {
                    StageStatus::Proposed
                } else {
                    StageStatus::Pending
                }
            }
        }
    }

    /// May the user open this stage? Earlier stages always; a later stage once
    /// the stage before it has been approved (or the stage already has content).
    pub fn can_open(&self, stage: Stage) -> bool {
        match stage.prev() {
            None => true,
            Some(prev) => self.status(prev) == StageStatus::Approved || !matches!(self.status(stage), StageStatus::Pending),
        }
    }

    /// After the user edits `stage`, every later stage that was produced from
    /// the old value becomes stale.
    pub fn invalidate_after(&mut self, stage: Stage) {
        let later = |s: Stage| s > stage;
        if later(Stage::Correct) {
            self.correction.mark_stale();
        }
        if later(Stage::Region) {
            self.regions.mark_stale();
        }
        let per_panel_only = stage.per_panel();
        let active = self.active_panel;
        for w in &mut self.panels {
            if per_panel_only && Some(w.panel_id) != active {
                continue;
            }
            if later(Stage::Text) {
                w.text.mark_stale();
            }
            if later(Stage::Axes) {
                w.axes.mark_stale();
            }
            if later(Stage::Data) {
                w.data.mark_stale();
            }
        }
    }

    /// Ensure every enabled panel has a work entry and drop entries for removed panels.
    pub fn sync_panels(&mut self) {
        let ids: Vec<u32> = self.panel_list().iter().map(|p| p.id).collect();
        self.panels.retain(|w| ids.contains(&w.panel_id));
        for id in &ids {
            self.work_mut(*id);
        }
        if self.active_panel.map(|a| !ids.contains(&a)).unwrap_or(true) {
            self.active_panel = self.panel_list().iter().find(|p| p.enabled).or(self.panel_list().first()).map(|p| p.id);
        }
    }

    pub fn next_panel_id(&self) -> u32 {
        self.panel_list().iter().map(|p| p.id + 1).max().unwrap_or(1)
    }
}

trait MaxWith {
    fn max_with(self, o: Self) -> Self;
}

impl MaxWith for StageStatus {
    fn max_with(self, o: Self) -> Self {
        if self == StageStatus::Pending {
            o
        } else {
            self
        }
    }
}

/// Encode bytes for embedding (used by exports).
pub fn to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_order_and_invalidation() {
        let mut p = Project::default();
        p.correction.propose(Correction::identity(10, 10), "");
        p.correction.approve();
        p.regions.propose(
            vec![Panel {
                id: 1,
                name: "Plot 1".into(),
                kind: AxesKind::Xy,
                inner: RectF::new(0.0, 0.0, 5.0, 5.0),
                outer: RectF::new(0.0, 0.0, 10.0, 10.0),
                enabled: true,
            }],
            "",
        );
        p.regions.approve();
        p.sync_panels();
        assert_eq!(p.active_panel, Some(1));
        p.work_mut(1).text.propose(vec![], "");
        p.work_mut(1).text.approve();
        p.invalidate_after(Stage::Correct);
        assert_eq!(p.regions.status, StageStatus::Stale);
        assert_eq!(p.work(1).unwrap().text.status, StageStatus::Stale);
        assert_eq!(p.correction.status, StageStatus::Approved);
    }

    #[test]
    fn json_roundtrip() {
        let mut p = Project::default();
        p.source = Some(SourceImage { name: "a.png".into(), width: 2, height: 3, bytes: vec![1, 2, 3, 255] });
        p.correction.propose(Correction::identity(2, 3), "none");
        let s = p.to_json();
        let q = Project::from_json(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn correction_identity_homography() {
        let c = Correction::identity(640, 480);
        let h = c.homography();
        let q = h.apply(Pt::new(100.0, 200.0));
        assert!(q.dist(Pt::new(100.0, 200.0)) < 1e-6);
    }
}
