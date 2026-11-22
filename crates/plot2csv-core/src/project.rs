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

    /// The axis-aligned rectangle a fine-tune turns `quad` (TL, TR, BR, BL)
    /// into: same centre, mean side lengths. Fails unless the corners form a
    /// convex shape in that order.
    pub fn fine_tune_target(quad: &[Pt; 4]) -> Result<RectF, String> {
        let convex = (0..4).all(|i| {
            let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
            b.sub(a).cross(c.sub(b)) > 0.0
        });
        if !convex {
            return Err("The corners must form a convex shape in the order TL, TR, BR, BL.".into());
        }
        let w = (quad[0].dist(quad[1]) + quad[3].dist(quad[2])) / 2.0;
        let h = (quad[0].dist(quad[3]) + quad[1].dist(quad[2])) / 2.0;
        if w < 8.0 || h < 8.0 {
            return Err("The corners are too close together.".into());
        }
        let c = quad.iter().fold(Pt::default(), |a, p| a.add(*p)).scale(0.25);
        Ok(RectF::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0))
    }

    /// How far (px) the farthest corner of `quad` is from its fine-tune target.
    pub fn fine_tune_shift(quad: &[Pt; 4]) -> Result<f64, String> {
        let target = Self::fine_tune_target(quad)?;
        Ok(quad.iter().zip(target.corners()).map(|(a, b)| a.dist(b)).fold(0.0, f64::max))
    }

    /// Correct further so that `quad` (TL, TR, BR, BL in the current corrected
    /// image) becomes an axis-aligned rectangle. The new correction still maps
    /// straight from the source image (no second resampling). `src_w × src_h`
    /// is the source size (the corrected image when correction is off).
    pub fn fine_tuned(&self, quad: &[Pt; 4], src_w: u32, src_h: u32) -> Result<FineTuned, String> {
        let target = Self::fine_tune_target(quad)?;
        let (old, ow, oh) = if self.enabled {
            (self.homography(), self.out_w as f64, self.out_h as f64)
        } else {
            (Homography::identity(), src_w as f64, src_h as f64)
        };
        let fix = Homography::fit(quad, &target.corners()).ok_or("Could not fit a perspective to these corners.")?;
        // The whole current image must stay on the same side of the new vanishing line.
        let side = fix.w(target.center()).signum();
        let image = RectF::new(0.0, 0.0, ow, oh);
        if image.corners().iter().any(|p| fix.w(*p) * side <= 0.0) {
            return Err("These corners ask for a stronger perspective change than the image allows.".into());
        }
        // New output: the re-corrected image plus the target, within a margin of the old frame.
        let margin = 0.1 * ow.max(oh);
        let limit = image.union(&target).expand(margin);
        let frame = fix.apply_rect(&image).union(&target).intersect(&limit).ok_or("The corners lie outside the image.")?;
        let (nw, nh) = (frame.width().round().max(16.0), frame.height().round().max(16.0));
        if nw > 8000.0 || nh > 8000.0 {
            return Err("The corrected image would be too large.".into());
        }
        let shift = Homography::translation(-frame.x0, -frame.y0);
        let map = fix.then(&shift);
        let to_source = old.then(&map).inverse().ok_or("Could not invert the new correction.")?;
        let out = RectF::new(0.0, 0.0, nw, nh).corners();
        let side = to_source.w(Pt::new(nw / 2.0, nh / 2.0)).signum();
        if out.iter().any(|p| to_source.w(*p) * side <= 0.0) {
            return Err("These corners ask for a stronger perspective change than the image allows.".into());
        }
        let correction = Correction {
            enabled: true,
            quad: out.map(|p| to_source.apply(p)),
            out_w: nw as u32,
            out_h: nh as u32,
            method: "plot area fine-tune".into(),
            confidence: self.confidence,
        };
        Ok(FineTuned { correction, map, rect: shift.apply_rect(&target) })
    }
}

/// Result of [`Correction::fine_tuned`].
#[derive(Clone, Debug, PartialEq)]
pub struct FineTuned {
    pub correction: Correction,
    /// Old corrected px -> new corrected px.
    pub map: Homography,
    /// Where the fine-tune corners land: an axis-aligned rectangle (new corrected px).
    pub rect: RectF,
}

/// Corners of a panel's data area being placed for a perspective fine-tune
/// (Plot area stage), not yet applied. Corrected px, order TL, TR, BR, BL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FineTune {
    pub panel: u32,
    pub quad: [Pt; 4],
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
    /// A perspective fine-tune being set up in the Plot area stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_tune: Option<FineTune>,
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
            fine_tune: None,
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

    /// Apply the pending fine-tune: correct the perspective again so that its
    /// corners become the panel's (rectangular) data area. The panels move with
    /// the image; text, axes and data become stale. `src_w × src_h` is the
    /// source image size. Returns how far (px) the farthest corner moved.
    pub fn apply_fine_tune(&mut self, src_w: u32, src_h: u32) -> Result<f64, String> {
        let ft = self.fine_tune.clone().ok_or("No fine-tune in progress.")?;
        let corr = self.correction.value.clone().unwrap_or_else(|| Correction::identity(src_w, src_h));
        let r = corr.fine_tuned(&ft.quad, src_w, src_h)?;
        let moved = Correction::fine_tune_shift(&ft.quad)?;
        let (w, h) = (r.correction.out_w as f64, r.correction.out_h as f64);
        for list in [self.regions.value.as_mut(), self.regions.auto.as_mut()].into_iter().flatten() {
            for p in list.iter_mut() {
                p.inner = r.map.apply_rect(&p.inner).clamp_to(w, h);
                p.outer = r.map.apply_rect(&p.outer).clamp_to(w, h);
            }
        }
        if let Some(p) = self.regions.value.iter_mut().flatten().find(|p| p.id == ft.panel) {
            p.inner = r.rect.clamp_to(w, h);
            p.outer = p.outer.union(&p.inner);
        }
        self.correction.value = Some(r.correction);
        self.fine_tune = None;
        self.invalidate_after(Stage::Region);
        Ok(moved)
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

    fn close(a: Pt, b: Pt, tol: f64) -> bool {
        a.dist(b) < tol
    }

    #[test]
    fn fine_tune_target_checks_the_corners() {
        let rect = RectF::new(10.0, 20.0, 110.0, 70.0);
        let t = Correction::fine_tune_target(&rect.corners()).unwrap();
        assert!(close(Pt::new(t.x0, t.y0), Pt::new(10.0, 20.0), 1e-9) && close(Pt::new(t.x1, t.y1), Pt::new(110.0, 70.0), 1e-9));
        assert_eq!(Correction::fine_tune_shift(&rect.corners()).unwrap(), 0.0);
        let [a, b, c, d] = rect.corners();
        assert!(Correction::fine_tune_target(&[a, c, b, d]).is_err(), "crossed");
        assert!(Correction::fine_tune_target(&[a, d, c, b]).is_err(), "counter-clockwise");
        assert!(Correction::fine_tune_target(&[a, a.add(Pt::new(5.0, 0.0)), a.add(Pt::new(5.0, 5.0)), a.add(Pt::new(0.0, 5.0))]).is_err());
    }

    /// Source points whose corrected positions are the fine-tune corners land
    /// on the corners of an axis-aligned rectangle after the fine-tune.
    fn check_fine_tune(corr: &Correction, src: (u32, u32), quad: [Pt; 4]) -> FineTuned {
        let r = corr.fine_tuned(&quad, src.0, src.1).unwrap();
        let old = if corr.enabled { corr.homography() } else { Homography::identity() };
        let old_inv = old.inverse().unwrap();
        let new = r.correction.homography();
        for (q, c) in quad.iter().zip(r.rect.corners()) {
            let p = new.apply(old_inv.apply(*q));
            assert!(close(p, c, 1e-6), "{p:?} vs {c:?}");
            assert!(close(r.map.apply(*q), c, 1e-6));
        }
        let t = Correction::fine_tune_target(&quad).unwrap();
        assert!((r.rect.width() - t.width()).abs() < 1e-9 && (r.rect.height() - t.height()).abs() < 1e-9);
        r
    }

    #[test]
    fn fine_tune_from_an_uncorrected_image() {
        let corr = Correction::identity(600, 400);
        // A frame that is a little narrower at the top.
        let quad = [Pt::new(104.0, 50.0), Pt::new(496.0, 52.0), Pt::new(510.0, 351.0), Pt::new(90.0, 349.0)];
        let r = check_fine_tune(&corr, (600, 400), quad);
        assert!(r.correction.enabled);
        assert_eq!(r.correction.method, "plot area fine-tune");
        assert!(r.correction.out_w.abs_diff(600) < 40 && r.correction.out_h.abs_diff(400) < 40, "{:?}", r.correction);
    }

    #[test]
    fn fine_tune_composes_with_the_existing_correction() {
        let mut corr = Correction::identity(800, 600);
        corr.enabled = true;
        corr.quad = [Pt::new(30.0, 10.0), Pt::new(780.0, 40.0), Pt::new(760.0, 590.0), Pt::new(10.0, 570.0)];
        corr.out_w = 760;
        corr.out_h = 560;
        let quad = [Pt::new(80.0, 40.0), Pt::new(700.0, 44.0), Pt::new(703.0, 500.0), Pt::new(78.0, 497.0)];
        check_fine_tune(&corr, (800, 600), quad);
    }

    #[test]
    fn rectangular_corners_keep_the_correction() {
        let mut corr = Correction::identity(800, 600);
        corr.enabled = true;
        corr.quad = [Pt::new(30.0, 10.0), Pt::new(780.0, 40.0), Pt::new(760.0, 590.0), Pt::new(10.0, 570.0)];
        corr.out_w = 760;
        corr.out_h = 560;
        let r = check_fine_tune(&corr, (800, 600), RectF::new(50.0, 60.0, 700.0, 500.0).corners());
        assert_eq!((r.correction.out_w, r.correction.out_h), (760, 560));
        for (a, b) in r.correction.quad.iter().zip(&corr.quad) {
            assert!(close(*a, *b, 1e-6), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn extreme_fine_tune_is_refused() {
        let corr = Correction::identity(600, 400);
        // Top edge squeezed to a few px: the vanishing line crosses the image.
        let quad = [Pt::new(298.0, 50.0), Pt::new(302.0, 50.0), Pt::new(590.0, 390.0), Pt::new(10.0, 390.0)];
        assert!(corr.fine_tuned(&quad, 600, 400).is_err());
    }

    #[test]
    fn apply_fine_tune_moves_panels_and_marks_later_stages_stale() {
        let mut p = Project::default();
        p.correction.propose(Correction::identity(600, 400), "");
        p.correction.approve();
        let panel = |id: u32, inner: RectF| Panel {
            id,
            name: format!("Plot {id}"),
            kind: AxesKind::Xy,
            inner,
            outer: inner.expand(20.0),
            enabled: true,
        };
        p.regions.propose(vec![panel(1, RectF::new(100.0, 50.0, 500.0, 350.0)), panel(2, RectF::new(520.0, 50.0, 590.0, 120.0))], "");
        p.sync_panels();
        p.work_mut(1).text.propose(vec![], "");
        let quad = [Pt::new(104.0, 50.0), Pt::new(496.0, 52.0), Pt::new(510.0, 351.0), Pt::new(90.0, 349.0)];
        p.fine_tune = Some(FineTune { panel: 1, quad });
        let moved = p.apply_fine_tune(600, 400).unwrap();
        assert!(moved > 5.0 && moved < 15.0, "{moved}");
        assert!(p.fine_tune.is_none());
        let c = p.correction.value.clone().unwrap();
        assert_eq!(p.correction.status, StageStatus::Approved);
        let (w, h) = (c.out_w as f64, c.out_h as f64);
        let r = c.homography();
        let p1 = p.panel(1).unwrap();
        for (q, corner) in quad.iter().zip(p1.inner.corners()) {
            assert!(close(r.apply(*q), corner, 1e-6));
        }
        assert!(p1.outer.contains(Pt::new(p1.inner.x0, p1.inner.y0)) && p1.outer.contains(Pt::new(p1.inner.x1, p1.inner.y1)));
        let moved2 = r.apply_rect(&RectF::new(520.0, 50.0, 590.0, 120.0)).clamp_to(w, h);
        let same = |a: &RectF| {
            close(Pt::new(a.x0, a.y0), Pt::new(moved2.x0, moved2.y0), 1e-6)
                && close(Pt::new(a.x1, a.y1), Pt::new(moved2.x1, moved2.y1), 1e-6)
        };
        assert!(same(&p.panel(2).unwrap().inner), "{:?} vs {moved2:?}", p.panel(2).unwrap().inner);
        assert!(same(&p.regions.auto.as_ref().unwrap()[1].inner), "the auto proposal moves too");
        assert_eq!(p.work(1).unwrap().text.status, StageStatus::Stale);
        assert!(p.apply_fine_tune(600, 400).is_err(), "nothing pending");
    }

    #[test]
    fn correction_identity_homography() {
        let c = Correction::identity(640, 480);
        let h = c.homography();
        let q = h.apply(Pt::new(100.0, 200.0));
        assert!(q.dist(Pt::new(100.0, 200.0)) < 1e-6);
    }
}
