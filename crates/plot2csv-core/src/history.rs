//! Undo/redo history of the project, kept as snapshots.
//!
//! The app calls [`History::track`] at the end of every frame in which nothing
//! is in flux (no drag, no focused text field, no running job). When the
//! project differs from the last snapshot, that snapshot becomes an undo step.
//! Snapshots leave out the source image (it only changes when a new document
//! is loaded, which resets the history) and the selection (active panel and
//! dataset): selecting something is not an edit.

use crate::project::{Project, Stage};

/// Undo steps kept; older ones are dropped.
pub const MAX_STEPS: usize = 100;
/// Arrow-key nudges closer together than this merge into one step.
const NUDGE_MERGE_SECS: f64 = 1.0;

/// A project snapshot (without the source image) and the stage shown with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub project: Project,
    pub stage: Stage,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// The state as of the last settled frame.
    baseline: Entry,
    /// Take the next tracked state as the baseline without committing (the
    /// first render after a restore may fill in values created on demand).
    absorb: bool,
    /// Time of the last commit made by an arrow-key nudge.
    last_nudge: Option<f64>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(&Project::default(), Stage::Import)
    }
}

/// Copy of `p` without the source image.
fn snapshot(p: &Project) -> Project {
    Project {
        version: p.version,
        source: None,
        import: p.import.clone(),
        correction: p.correction.clone(),
        regions: p.regions.clone(),
        panels: p.panels.clone(),
        active_panel: p.active_panel,
    }
}

/// Everything in a project except the source image and the selection.
fn content_view(p: &Project) -> impl PartialEq + serde::Serialize + '_ {
    let Project { version, source: _, import, correction, regions, panels, active_panel: _ } = p;
    let panels: Vec<_> = panels
        .iter()
        .map(|w| {
            let crate::PanelWork { panel_id, text, axes, evidence, data, measurements, active_dataset: _ } = w;
            (panel_id, text, axes, evidence, data, measurements)
        })
        .collect();
    (version, import, correction, regions, panels)
}

/// Same content (ignoring source and selection)? A difference reported by `==`
/// is confirmed through JSON, which treats NaN as equal to itself.
fn content_eq(a: &Project, b: &Project) -> bool {
    let (va, vb) = (content_view(a), content_view(b));
    va == vb || serde_json::to_string(&va).ok() == serde_json::to_string(&vb).ok()
}

fn selection_eq(a: &Project, b: &Project) -> bool {
    a.active_panel == b.active_panel
        && a.panels.len() == b.panels.len()
        && a.panels.iter().zip(&b.panels).all(|(x, y)| x.active_dataset == y.active_dataset)
}

impl History {
    pub fn new(p: &Project, stage: Stage) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), baseline: Entry { project: snapshot(p), stage }, absorb: false, last_nudge: None }
    }

    /// Start over with a new document. Its first settled frame becomes the
    /// baseline (the first render may fill in values created on demand).
    pub fn reset(&mut self, p: &Project, stage: Stage) {
        *self = Self::new(p, stage);
        self.absorb = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Record the state of a settled frame. `nudge_at` is the current time if
    /// this frame's change came from an arrow-key nudge.
    pub fn track(&mut self, p: &Project, stage: Stage, nudge_at: Option<f64>) {
        if self.absorb {
            self.absorb = false;
            self.baseline = Entry { project: snapshot(p), stage };
            return;
        }
        if content_eq(p, &self.baseline.project) {
            if !selection_eq(p, &self.baseline.project) {
                self.baseline.project = snapshot(p);
            }
            self.baseline.stage = stage;
            return;
        }
        let merge = match (nudge_at, self.last_nudge) {
            (Some(now), Some(last)) => now - last < NUDGE_MERGE_SECS && self.can_undo(),
            _ => false,
        };
        self.commit(p, stage, merge);
        self.last_nudge = nudge_at;
    }

    fn commit(&mut self, p: &Project, stage: Stage, merge: bool) {
        let prev = std::mem::replace(&mut self.baseline, Entry { project: snapshot(p), stage });
        if !merge {
            self.undo.push(prev);
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Commit a change that has not been tracked yet.
    fn flush(&mut self, p: &Project, stage: Stage) {
        if !self.absorb && !content_eq(p, &self.baseline.project) {
            self.commit(p, stage, false);
        }
        self.last_nudge = None;
    }

    /// Step back. Returns the snapshot to restore (its `source` is `None`).
    pub fn undo(&mut self, p: &Project, stage: Stage) -> Option<Entry> {
        self.flush(p, stage);
        let e = self.undo.pop()?;
        self.redo.push(Entry { project: snapshot(p), stage });
        Some(self.restored(e))
    }

    /// Step forward again. Returns the snapshot to restore (its `source` is `None`).
    pub fn redo(&mut self, p: &Project, stage: Stage) -> Option<Entry> {
        self.flush(p, stage);
        let e = self.redo.pop()?;
        self.undo.push(Entry { project: snapshot(p), stage });
        Some(self.restored(e))
    }

    fn restored(&mut self, e: Entry) -> Entry {
        self.baseline = e.clone();
        self.absorb = true;
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Correction, SourceImage};
    use crate::PanelWork;

    fn project() -> Project {
        let mut p = Project {
            source: Some(SourceImage { name: "a.png".into(), width: 10, height: 10, bytes: vec![1, 2, 3] }),
            ..Default::default()
        };
        p.correction.propose(Correction::identity(10, 10), "");
        p.panels.push(PanelWork::new(1));
        p.active_panel = Some(1);
        p
    }

    /// Restore an entry the way the app does: keep the live source image.
    fn apply(p: &mut Project, e: Entry) -> Stage {
        let src = p.source.take();
        *p = e.project;
        p.source = src;
        e.stage
    }

    #[test]
    fn commits_only_changes() {
        let mut p = project();
        let mut h = History::new(&p, Stage::Correct);
        h.track(&p, Stage::Correct, None);
        assert!(!h.can_undo());
        p.correction.value.as_mut().unwrap().out_w = 20;
        h.track(&p, Stage::Correct, None);
        assert!(h.can_undo());
        h.track(&p, Stage::Correct, None);
        assert_eq!(h.undo.len(), 1);
    }

    #[test]
    fn selection_is_not_a_step_but_is_restored() {
        let mut p = project();
        p.panels.push(PanelWork::new(2));
        let mut h = History::new(&p, Stage::Data);
        p.active_panel = Some(2);
        p.panels[1].active_dataset = 3;
        h.track(&p, Stage::Data, None);
        assert!(!h.can_undo());
        p.panels[1].measurements.distances.push([crate::Pt::new(0.0, 0.0), crate::Pt::new(1.0, 1.0)]);
        h.track(&p, Stage::Data, None);
        let e = h.undo(&p, Stage::Data).unwrap();
        assert_eq!(e.project.active_panel, Some(2));
        assert_eq!(e.project.panels[1].active_dataset, 3);
        assert!(e.project.panels[1].measurements.distances.is_empty());
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut p = project();
        let before = p.clone();
        let mut h = History::new(&p, Stage::Correct);
        p.correction.approve();
        p.regions.propose(vec![], "");
        h.track(&p, Stage::Region, None);
        let after = p.clone();

        let e = h.undo(&p, Stage::Region).unwrap();
        let stage = apply(&mut p, e);
        assert_eq!((&p, stage), (&before, Stage::Correct));
        assert!(h.can_redo());
        h.track(&p, stage, None); // the absorbed first frame after a restore
        assert!(h.can_redo());

        let e = h.redo(&p, stage).unwrap();
        let stage = apply(&mut p, e);
        assert_eq!((&p, stage), (&after, Stage::Region));
        assert!(h.undo(&p, stage).is_some());
        assert!(h.undo(&p, Stage::Correct).is_none());
    }

    #[test]
    fn new_change_clears_redo() {
        let mut p = project();
        let mut h = History::new(&p, Stage::Correct);
        p.correction.approve();
        h.track(&p, Stage::Correct, None);
        let e = h.undo(&p, Stage::Correct).unwrap();
        apply(&mut p, e);
        h.track(&p, Stage::Correct, None);
        p.correction.value.as_mut().unwrap().out_h = 5;
        h.track(&p, Stage::Correct, None);
        assert!(!h.can_redo());
    }

    #[test]
    fn untracked_change_is_flushed_by_undo() {
        let mut p = project();
        let before = p.clone();
        let mut h = History::new(&p, Stage::Correct);
        p.correction.approve();
        let e = h.undo(&p, Stage::Correct).unwrap();
        apply(&mut p, e);
        assert_eq!(p, before);
    }

    #[test]
    fn nudges_merge_within_a_second() {
        let mut p = project();
        let mut h = History::new(&p, Stage::Correct);
        for (i, t) in [10.0, 10.2, 10.4, 10.6].iter().enumerate() {
            p.correction.value.as_mut().unwrap().quad[0].x = i as f64 + 1.0;
            h.track(&p, Stage::Correct, Some(*t));
        }
        assert_eq!(h.undo.len(), 1);
        p.correction.value.as_mut().unwrap().quad[0].x = 9.0;
        h.track(&p, Stage::Correct, Some(12.0));
        assert_eq!(h.undo.len(), 2);
        p.correction.value.as_mut().unwrap().quad[0].x = 10.0;
        h.track(&p, Stage::Correct, None);
        assert_eq!(h.undo.len(), 3);
    }

    #[test]
    fn capped() {
        let mut p = project();
        let mut h = History::new(&p, Stage::Correct);
        for i in 0..MAX_STEPS + 20 {
            p.correction.value.as_mut().unwrap().out_w = i as u32 + 100;
            h.track(&p, Stage::Correct, None);
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
    }

    #[test]
    fn nan_does_not_flood() {
        let mut p = project();
        p.correction.value.as_mut().unwrap().confidence = f64::NAN;
        let mut h = History::new(&p, Stage::Correct);
        for _ in 0..5 {
            h.track(&p, Stage::Correct, None);
        }
        assert!(!h.can_undo());
    }

    #[test]
    fn snapshots_leave_out_the_source() {
        let mut p = project();
        let mut h = History::new(&p, Stage::Correct);
        assert!(h.baseline.project.source.is_none());
        p.correction.approve();
        h.track(&p, Stage::Correct, None);
        assert!(h.undo.iter().all(|e| e.project.source.is_none()));
        assert!(h.undo(&p, Stage::Correct).unwrap().project.source.is_none());
        assert!(h.redo.iter().all(|e| e.project.source.is_none()));
    }
}
