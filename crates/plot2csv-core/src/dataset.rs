//! Datasets (series), their points, and per-dataset automatic-extraction settings.

use serde::{Deserialize, Serialize};

use crate::geom::Pt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataPoint {
    /// Corrected-image pixel coordinates.
    pub x: f64,
    pub y: f64,
    /// Category label (bar charts) or free-form tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl DataPoint {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y, label: None }
    }
    pub fn pt(&self) -> Pt {
        Pt::new(self.x, self.y)
    }
}

/// How a series is drawn in the source plot (detected or chosen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SeriesStyle {
    #[default]
    Line,
    Dashed,
    Markers,
    Bars,
}

impl SeriesStyle {
    pub const ALL: [SeriesStyle; 4] = [SeriesStyle::Line, SeriesStyle::Dashed, SeriesStyle::Markers, SeriesStyle::Bars];
    pub fn label(self) -> &'static str {
        match self {
            SeriesStyle::Line => "Line",
            SeriesStyle::Dashed => "Dashed line",
            SeriesStyle::Markers => "Markers",
            SeriesStyle::Bars => "Bars",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorMode {
    /// Pixels close to the foreground colour are data.
    #[default]
    Foreground,
    /// Pixels far from the background colour are data.
    Background,
}

/// Automatic extraction algorithm and its parameters (WebPlotDigitizer names).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Algorithm {
    /// Averaging window: ΔX, ΔY in pixels.
    AveragingWindow { dx: f64, dy: f64 },
    /// X step with interpolation, in data units; smoothing in % of the step.
    XStepInterp { xmin: f64, xstep: f64, xmax: f64, ymin: f64, ymax: f64, smoothing: f64 },
    /// X step (no interpolation), in data units.
    XStep { xmin: f64, xstep: f64, xmax: f64, ymin: f64, ymax: f64 },
    /// Blob detector: diameters in pixels.
    Blob { min_d: f64, max_d: f64 },
    /// Bar extraction: tolerance in pixels for merging columns.
    Bar { min_width: f64 },
    /// Multi-track: separate several same-colour curves (dashed/solid) as functions of x.
    MultiTrack { tracks: usize, max_jump: f64, max_gap: f64 },
}

impl Default for Algorithm {
    fn default() -> Self {
        Algorithm::AveragingWindow { dx: 10.0, dy: 10.0 }
    }
}

impl Algorithm {
    pub fn label(&self) -> &'static str {
        match self {
            Algorithm::AveragingWindow { .. } => "Averaging Window",
            Algorithm::XStepInterp { .. } => "X Step w/ Interp.",
            Algorithm::XStep { .. } => "X Step",
            Algorithm::Blob { .. } => "Blob Detector",
            Algorithm::Bar { .. } => "Bar Extraction",
            Algorithm::MultiTrack { .. } => "Multi-track (same colour)",
        }
    }
}

/// A binary mask stored as alternating run lengths (first run is "off").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub runs: Vec<u32>,
}

impl Mask {
    pub fn from_bits(width: u32, height: u32, bits: &[bool]) -> Self {
        let mut runs = Vec::new();
        let mut cur = false;
        let mut n = 0u32;
        for &b in bits {
            if b == cur {
                n += 1;
            } else {
                runs.push(n);
                cur = b;
                n = 1;
            }
        }
        runs.push(n);
        Self { width, height, runs }
    }

    pub fn to_bits(&self) -> Vec<bool> {
        let mut v = Vec::with_capacity((self.width * self.height) as usize);
        let mut cur = false;
        for &r in &self.runs {
            v.extend(std::iter::repeat_n(cur, r as usize));
            cur = !cur;
        }
        v.resize((self.width * self.height) as usize, false);
        v
    }

    pub fn is_empty(&self) -> bool {
        self.runs.len() <= 1
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoSettings {
    pub color_mode: ColorMode,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    /// Colour distance tolerance (RGB Euclidean), WPD default 120.
    pub distance: f64,
    pub algorithm: Algorithm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
}

impl Default for AutoSettings {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::Foreground,
            fg: [0, 0, 200],
            bg: [255, 255, 255],
            distance: 120.0,
            algorithm: Algorithm::default(),
            mask: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    pub name: String,
    /// Colour used to draw the dataset (usually the detected series colour).
    pub color: [u8; 3],
    pub points: Vec<DataPoint>,
    pub style: SeriesStyle,
    pub visible: bool,
    pub auto: AutoSettings,
}

impl Dataset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: [220, 40, 40],
            points: Vec::new(),
            style: SeriesStyle::Line,
            visible: true,
            auto: AutoSettings::default(),
        }
    }

    /// Index of the point nearest to `p` within `max_dist` pixels.
    pub fn nearest(&self, p: Pt, max_dist: f64) -> Option<usize> {
        let mut best = None;
        let mut bd = max_dist;
        for (i, q) in self.points.iter().enumerate() {
            let d = q.pt().dist(p);
            if d <= bd {
                bd = d;
                best = Some(i);
            }
        }
        best
    }
}

/// Distinct colours for new datasets (Okabe–Ito palette, colour-blind safe).
pub const PALETTE: [[u8; 3]; 8] = [
    [230, 159, 0],
    [86, 180, 233],
    [0, 158, 115],
    [240, 228, 66],
    [0, 114, 178],
    [213, 94, 0],
    [204, 121, 167],
    [120, 120, 120],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_rle_roundtrip() {
        let bits = vec![false, false, true, true, true, false, true, false, false];
        let m = Mask::from_bits(3, 3, &bits);
        assert_eq!(m.to_bits(), bits);
        let m2 = Mask::from_bits(2, 1, &[true, true]);
        assert_eq!(m2.runs, vec![0, 2]);
        assert_eq!(m2.to_bits(), vec![true, true]);
    }
}
