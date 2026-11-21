//! Axis calibrations (pixel <-> data) for every plot type, following
//! WebPlotDigitizer's conventions.

use serde::{Deserialize, Serialize};

use crate::geom::Pt;
use crate::value::{format_date, format_number, parse_value, NumFormat, Parsed};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AxesKind {
    #[default]
    Xy,
    Bar,
    Polar,
    Ternary,
    Map,
    Image,
}

impl AxesKind {
    pub const ALL: [AxesKind; 6] =
        [AxesKind::Xy, AxesKind::Bar, AxesKind::Polar, AxesKind::Ternary, AxesKind::Map, AxesKind::Image];

    pub fn label(self) -> &'static str {
        match self {
            AxesKind::Xy => "2D (X-Y) Plot",
            AxesKind::Bar => "Bar Chart",
            AxesKind::Polar => "Polar Diagram",
            AxesKind::Ternary => "Ternary Diagram",
            AxesKind::Map => "Map With Scale Bar",
            AxesKind::Image => "Image",
        }
    }

    /// Names of the calibration points, in the order they are clicked.
    pub fn point_names(self) -> &'static [&'static str] {
        match self {
            AxesKind::Xy => &["X1", "X2", "Y1", "Y2"],
            AxesKind::Bar => &["P1", "P2"],
            AxesKind::Polar => &["Origin", "P1", "P2"],
            AxesKind::Ternary => &["A", "B", "C"],
            AxesKind::Map => &["P1", "P2"],
            AxesKind::Image => &[],
        }
    }

    /// Instructions shown while placing calibration points.
    pub fn point_help(self) -> &'static str {
        match self {
            AxesKind::Xy => "Click four known points on the axes in order: two on the X axis (X1, X2) and two on the Y axis (Y1, Y2). Pick points far apart for accuracy.",
            AxesKind::Bar => "Click two known points on the value axis (P1, P2), e.g. two labelled ticks.",
            AxesKind::Polar => "Click the centre (origin), followed by two points with known (r, θ).",
            AxesKind::Ternary => "Click the three corners of the triangle: bottom-left (A), bottom-right (B), top (C).",
            AxesKind::Map => "Click the two ends of the scale bar.",
            AxesKind::Image => "No calibration needed: values are image pixels.",
        }
    }

    /// Kinds whose scale can be inferred automatically from ticks and labels.
    pub fn auto_calibratable(self) -> bool {
        matches!(self, AxesKind::Xy | AxesKind::Bar)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XyParams {
    pub x1: String,
    pub x2: String,
    pub y1: String,
    pub y2: String,
    pub log_x: bool,
    pub log_y: bool,
    /// Output format for date axes (WPD tokens).
    pub date_format: String,
}

impl Default for XyParams {
    fn default() -> Self {
        Self {
            x1: "0".into(),
            x2: "1".into(),
            y1: "0".into(),
            y2: "1".into(),
            log_x: false,
            log_y: false,
            date_format: "yyyy/mm/dd".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BarParams {
    pub p1: String,
    pub p2: String,
    pub log: bool,
}

impl Default for BarParams {
    fn default() -> Self {
        Self { p1: "0".into(), p2: "1".into(), log: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolarParams {
    pub r1: String,
    pub theta1: String,
    pub r2: String,
    pub theta2: String,
    pub degrees: bool,
    pub clockwise: bool,
    pub log_r: bool,
}

impl Default for PolarParams {
    fn default() -> Self {
        Self {
            r1: "1".into(),
            theta1: "0".into(),
            r2: "2".into(),
            theta2: "90".into(),
            degrees: true,
            clockwise: false,
            log_r: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TernaryParams {
    pub range100: bool,
    pub normal: bool,
}

impl Default for TernaryParams {
    fn default() -> Self {
        Self { range100: false, normal: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapParams {
    pub length: String,
    pub units: String,
}

impl Default for MapParams {
    fn default() -> Self {
        Self { length: "1".into(), units: "Units".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AxesParams {
    Xy(XyParams),
    Bar(BarParams),
    Polar(PolarParams),
    Ternary(TernaryParams),
    Map(MapParams),
    Image,
}

/// A calibration as edited by the user: point positions plus parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Axes {
    pub kind: AxesKind,
    /// Pixel positions (corrected-image px), `kind.point_names()` order.
    pub points: Vec<Pt>,
    pub params: AxesParams,
    /// Height of the image, needed by map axes (origin at the bottom-left).
    pub image_height: f64,
}

impl Axes {
    pub fn new(kind: AxesKind, image_height: f64) -> Self {
        let params = match kind {
            AxesKind::Xy => AxesParams::Xy(XyParams::default()),
            AxesKind::Bar => AxesParams::Bar(BarParams::default()),
            AxesKind::Polar => AxesParams::Polar(PolarParams::default()),
            AxesKind::Ternary => AxesParams::Ternary(TernaryParams::default()),
            AxesKind::Map => AxesParams::Map(MapParams::default()),
            AxesKind::Image => AxesParams::Image,
        };
        Self { kind, points: Vec::new(), params, image_height }
    }

    pub fn points_needed(&self) -> usize {
        self.kind.point_names().len()
    }

    pub fn points_complete(&self) -> bool {
        self.points.len() >= self.points_needed()
    }

    /// Build the transform; errors explain what is missing or inconsistent.
    pub fn calibrate(&self) -> Result<Calibration, String> {
        if !self.points_complete() {
            return Err(format!(
                "place {} more calibration point(s)",
                self.points_needed() - self.points.len()
            ));
        }
        let p = &self.points;
        match &self.params {
            AxesParams::Xy(x) => XyCal::new(p[0], p[1], p[2], p[3], x).map(Calibration::Xy),
            AxesParams::Bar(b) => BarCal::new(p[0], p[1], b).map(Calibration::Bar),
            AxesParams::Polar(pp) => PolarCal::new(p[0], p[1], p[2], pp).map(Calibration::Polar),
            AxesParams::Ternary(t) => TernaryCal::new(p[0], p[1], p[2], t).map(Calibration::Ternary),
            AxesParams::Map(m) => MapCal::new(p[0], p[1], m, self.image_height).map(Calibration::Map),
            AxesParams::Image => Ok(Calibration::Image),
        }
    }
}

fn val(s: &str, what: &str) -> Result<Parsed, String> {
    parse_value(s).ok_or_else(|| format!("{what}: cannot read '{s}' as a number or date (yyyy/mm/dd)"))
}

fn num(s: &str, what: &str) -> Result<f64, String> {
    match val(s, what)? {
        Parsed::Number(v) => Ok(v),
        Parsed::Date(_) => Err(format!("{what}: dates are not allowed here")),
    }
}

fn to_log(v: f64, log: bool, what: &str) -> Result<f64, String> {
    if !log {
        return Ok(v);
    }
    if v <= 0.0 {
        return Err(format!("{what}: log scale needs positive values"));
    }
    Ok(v.log10())
}

#[derive(Clone, Debug)]
pub struct XyCal {
    a: [[f64; 2]; 2],
    a_inv: [[f64; 2]; 2],
    c: [f64; 2],
    pub log_x: bool,
    pub log_y: bool,
    pub x_date: bool,
    pub y_date: bool,
    pub date_format: String,
}

impl XyCal {
    fn new(x1: Pt, x2: Pt, y1: Pt, y2: Pt, p: &XyParams) -> Result<Self, String> {
        let vx1 = val(&p.x1, "X1")?;
        let vx2 = val(&p.x2, "X2")?;
        let vy1 = val(&p.y1, "Y1")?;
        let vy2 = val(&p.y2, "Y2")?;
        let xmin = to_log(vx1.value(), p.log_x, "X1")?;
        let xmax = to_log(vx2.value(), p.log_x, "X2")?;
        let ymin = to_log(vy1.value(), p.log_y, "Y1")?;
        let ymax = to_log(vy2.value(), p.log_y, "Y2")?;
        if xmin == xmax {
            return Err("X1 and X2 must have different values".into());
        }
        if ymin == ymax {
            return Err("Y1 and Y2 must have different values".into());
        }
        // WebPlotDigitizer: A * [x1-x2, y1-y2] = [[xmin-xmax, 0], [0, ymin-ymax]]
        let pm = [[x1.x - x2.x, y1.x - y2.x], [x1.y - x2.y, y1.y - y2.y]];
        let det = pm[0][0] * pm[1][1] - pm[0][1] * pm[1][0];
        if det.abs() < 1e-9 {
            return Err("the X and Y calibration points are collinear; move them apart".into());
        }
        let pinv = [[pm[1][1] / det, -pm[0][1] / det], [-pm[1][0] / det, pm[0][0] / det]];
        let dm = [[xmin - xmax, 0.0], [0.0, ymin - ymax]];
        let a = mul2(&dm, &pinv);
        let c = [xmin - a[0][0] * x1.x - a[0][1] * x1.y, ymin - a[1][0] * y1.x - a[1][1] * y1.y];
        let ad = a[0][0] * a[1][1] - a[0][1] * a[1][0];
        let a_inv = [[a[1][1] / ad, -a[0][1] / ad], [-a[1][0] / ad, a[0][0] / ad]];
        Ok(Self {
            a,
            a_inv,
            c,
            log_x: p.log_x,
            log_y: p.log_y,
            x_date: vx1.is_date() && vx2.is_date(),
            y_date: vy1.is_date() && vy2.is_date(),
            date_format: p.date_format.clone(),
        })
    }

    pub fn to_data(&self, p: Pt) -> [f64; 2] {
        let x = self.a[0][0] * p.x + self.a[0][1] * p.y + self.c[0];
        let y = self.a[1][0] * p.x + self.a[1][1] * p.y + self.c[1];
        [if self.log_x { 10f64.powf(x) } else { x }, if self.log_y { 10f64.powf(y) } else { y }]
    }

    pub fn to_pixel(&self, x: f64, y: f64) -> Option<Pt> {
        let x = if self.log_x { (x > 0.0).then(|| x.log10())? } else { x };
        let y = if self.log_y { (y > 0.0).then(|| y.log10())? } else { y };
        let (dx, dy) = (x - self.c[0], y - self.c[1]);
        Some(Pt::new(self.a_inv[0][0] * dx + self.a_inv[0][1] * dy, self.a_inv[1][0] * dx + self.a_inv[1][1] * dy))
    }

    /// Text form of the transform for the "Transformation equations" panel.
    pub fn equations(&self) -> String {
        let fx = if self.log_x { "log10(x)" } else { "x" };
        let fy = if self.log_y { "log10(y)" } else { "y" };
        format!(
            "{fx} = {:.6e}·px + {:.6e}·py + {:.6e}\n{fy} = {:.6e}·px + {:.6e}·py + {:.6e}",
            self.a[0][0], self.a[0][1], self.c[0], self.a[1][0], self.a[1][1], self.c[1]
        )
    }
}

fn mul2(a: &[[f64; 2]; 2], b: &[[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [
        [a[0][0] * b[0][0] + a[0][1] * b[1][0], a[0][0] * b[0][1] + a[0][1] * b[1][1]],
        [a[1][0] * b[0][0] + a[1][1] * b[1][0], a[1][0] * b[0][1] + a[1][1] * b[1][1]],
    ]
}

#[derive(Clone, Debug)]
pub struct BarCal {
    pub p1: Pt,
    pub p2: Pt,
    v1: f64,
    v2: f64,
    pub log: bool,
}

impl BarCal {
    fn new(p1: Pt, p2: Pt, b: &BarParams) -> Result<Self, String> {
        let v1 = to_log(num(&b.p1, "P1")?, b.log, "P1")?;
        let v2 = to_log(num(&b.p2, "P2")?, b.log, "P2")?;
        if v1 == v2 {
            return Err("P1 and P2 must have different values".into());
        }
        if p1.dist(p2) < 1.0 {
            return Err("P1 and P2 are too close".into());
        }
        Ok(Self { p1, p2, v1, v2, log: b.log })
    }

    /// Bars are vertical when the value axis is vertical.
    pub fn vertical_bars(&self) -> bool {
        (self.p2.y - self.p1.y).abs() >= (self.p2.x - self.p1.x).abs()
    }

    pub fn value(&self, p: Pt) -> f64 {
        let d = self.p2.sub(self.p1);
        let t = p.sub(self.p1).dot(d) / d.dot(d);
        let v = self.v1 + t * (self.v2 - self.v1);
        if self.log {
            10f64.powf(v)
        } else {
            v
        }
    }

    /// Pixel position along the value axis for a value (the base point for drawing).
    pub fn axis_point(&self, value: f64) -> Option<Pt> {
        let v = if self.log { (value > 0.0).then(|| value.log10())? } else { value };
        let t = (v - self.v1) / (self.v2 - self.v1);
        Some(self.p1.lerp(self.p2, t))
    }
}

#[derive(Clone, Debug)]
pub struct PolarCal {
    pub origin: Pt,
    r0: f64,
    scale: f64,
    offset: f64,
    pub degrees: bool,
    pub clockwise: bool,
    pub log_r: bool,
}

impl PolarCal {
    fn new(o: Pt, p1: Pt, p2: Pt, p: &PolarParams) -> Result<Self, String> {
        let r1 = to_log(num(&p.r1, "R1")?, p.log_r, "R1")?;
        let r2 = to_log(num(&p.r2, "R2")?, p.log_r, "R2")?;
        let mut t1 = num(&p.theta1, "θ1")?;
        let _t2 = num(&p.theta2, "θ2")?;
        if p.degrees {
            t1 = t1.to_radians();
        }
        let d1 = p1.dist(o);
        let d2 = p2.dist(o);
        if (d2 - d1).abs() < 1e-6 {
            return Err("P1 and P2 must be at different distances from the origin".into());
        }
        if r1 == r2 {
            return Err("R1 and R2 must differ".into());
        }
        let scale = (r2 - r1) / (d2 - d1);
        let r0 = r1 - d1 * scale;
        let mut cal = Self { origin: o, r0, scale, offset: 0.0, degrees: p.degrees, clockwise: p.clockwise, log_r: p.log_r };
        cal.offset = t1 - cal.screen_angle(p1);
        Ok(cal)
    }

    fn screen_angle(&self, p: Pt) -> f64 {
        let a = (-(p.y - self.origin.y)).atan2(p.x - self.origin.x);
        if self.clockwise {
            -a
        } else {
            a
        }
    }

    pub fn to_data(&self, p: Pt) -> [f64; 2] {
        let r = self.r0 + p.dist(self.origin) * self.scale;
        let r = if self.log_r { 10f64.powf(r) } else { r };
        let t = (self.screen_angle(p) + self.offset).rem_euclid(std::f64::consts::TAU);
        [r, if self.degrees { t.to_degrees() } else { t }]
    }

    pub fn to_pixel(&self, r: f64, theta: f64) -> Option<Pt> {
        let r = if self.log_r { (r > 0.0).then(|| r.log10())? } else { r };
        let d = (r - self.r0) / self.scale;
        let t = if self.degrees { theta.to_radians() } else { theta };
        let mut a = t - self.offset;
        if self.clockwise {
            a = -a;
        }
        Some(Pt::new(self.origin.x + d * a.cos(), self.origin.y - d * a.sin()))
    }
}

#[derive(Clone, Debug)]
pub struct TernaryCal {
    pub a: Pt,
    pub b: Pt,
    pub c: Pt,
    det: f64,
    pub range100: bool,
    pub normal: bool,
}

impl TernaryCal {
    fn new(a: Pt, b: Pt, c: Pt, t: &TernaryParams) -> Result<Self, String> {
        let det = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
        if det.abs() < 1e-6 {
            return Err("the three corners are collinear".into());
        }
        Ok(Self { a, b, c, det, range100: t.range100, normal: t.normal })
    }

    fn bary(&self, p: Pt) -> [f64; 3] {
        let (a, b, c) = (self.a, self.b, self.c);
        let la = ((b.y - c.y) * (p.x - c.x) + (c.x - b.x) * (p.y - c.y)) / self.det;
        let lb = ((c.y - a.y) * (p.x - c.x) + (a.x - c.x) * (p.y - c.y)) / self.det;
        [la, lb, 1.0 - la - lb]
    }

    pub fn to_data(&self, p: Pt) -> [f64; 3] {
        let [la, lb, lc] = self.bary(p);
        let v = if self.normal { [la, lb, lc] } else { [lc, la, lb] };
        let s = if self.range100 { 100.0 } else { 1.0 };
        [v[0] * s, v[1] * s, v[2] * s]
    }

    pub fn to_pixel(&self, d: [f64; 3]) -> Pt {
        let s = if self.range100 { 100.0 } else { 1.0 };
        let v = [d[0] / s, d[1] / s, d[2] / s];
        let [la, lb, lc] = if self.normal { v } else { [v[1], v[2], v[0]] };
        Pt::new(
            la * self.a.x + lb * self.b.x + lc * self.c.x,
            la * self.a.y + lb * self.b.y + lc * self.c.y,
        )
    }
}

#[derive(Clone, Debug)]
pub struct MapCal {
    pub scale: f64,
    pub units: String,
    pub image_height: f64,
}

impl MapCal {
    fn new(p1: Pt, p2: Pt, m: &MapParams, image_height: f64) -> Result<Self, String> {
        let len = num(&m.length, "Scale length")?;
        let d = p1.dist(p2);
        if d < 1.0 {
            return Err("the scale bar end points are too close".into());
        }
        if len <= 0.0 {
            return Err("the scale length must be positive".into());
        }
        Ok(Self { scale: len / d, units: m.units.clone(), image_height })
    }
}

/// A usable calibration.
#[derive(Clone, Debug)]
pub enum Calibration {
    Xy(XyCal),
    Bar(BarCal),
    Polar(PolarCal),
    Ternary(TernaryCal),
    Map(MapCal),
    Image,
}

impl Calibration {
    /// Column names of the data table.
    pub fn variables(&self) -> Vec<String> {
        match self {
            Calibration::Xy(_) | Calibration::Image => vec!["X".into(), "Y".into()],
            Calibration::Map(m) => vec![format!("X ({})", m.units), format!("Y ({})", m.units)],
            Calibration::Bar(_) => vec!["Label".into(), "Value".into()],
            Calibration::Polar(p) => vec!["r".into(), if p.degrees { "θ (°)".into() } else { "θ (rad)".into() }],
            Calibration::Ternary(_) => vec!["a".into(), "b".into(), "c".into()],
        }
    }

    /// Numeric values for a pixel position (for Bar: just the value).
    pub fn to_data(&self, p: Pt) -> Vec<f64> {
        match self {
            Calibration::Xy(c) => c.to_data(p).to_vec(),
            Calibration::Bar(c) => vec![c.value(p)],
            Calibration::Polar(c) => c.to_data(p).to_vec(),
            Calibration::Ternary(c) => c.to_data(p).to_vec(),
            Calibration::Map(c) => vec![p.x * c.scale, (c.image_height - p.y) * c.scale],
            Calibration::Image => vec![p.x, p.y],
        }
    }

    /// Inverse transform where it is defined (not for bar values).
    pub fn to_pixel(&self, d: &[f64]) -> Option<Pt> {
        match self {
            Calibration::Xy(c) => c.to_pixel(*d.first()?, *d.get(1)?),
            Calibration::Polar(c) => c.to_pixel(*d.first()?, *d.get(1)?),
            Calibration::Ternary(c) => Some(c.to_pixel([*d.first()?, *d.get(1)?, *d.get(2)?])),
            Calibration::Map(c) => Some(Pt::new(d.first()? / c.scale, c.image_height - d.get(1)? / c.scale)),
            Calibration::Image => Some(Pt::new(*d.first()?, *d.get(1)?)),
            Calibration::Bar(_) => None,
        }
    }

    /// Is data column `i` a date?
    pub fn is_date(&self, i: usize) -> bool {
        match self {
            Calibration::Xy(c) => (i == 0 && c.x_date) || (i == 1 && c.y_date),
            _ => false,
        }
    }

    pub fn date_format(&self) -> &str {
        match self {
            Calibration::Xy(c) => &c.date_format,
            _ => "yyyy/mm/dd",
        }
    }

    /// Format one value of column `i` for display.
    pub fn format_value(&self, i: usize, v: f64, fmt: NumFormat, digits: usize) -> String {
        if self.is_date(i) {
            format_date(v, self.date_format())
        } else {
            format_number(v, fmt, digits)
        }
    }

    /// Distance between two pixel positions in calibrated units, with a unit label.
    pub fn distance(&self, a: Pt, b: Pt) -> (f64, String) {
        match self {
            Calibration::Map(m) => (a.dist(b) * m.scale, m.units.clone()),
            Calibration::Xy(c) if !c.log_x && !c.log_y => {
                let (da, db) = (c.to_data(a), c.to_data(b));
                (((da[0] - db[0]).powi(2) + (da[1] - db[1]).powi(2)).sqrt(), String::new())
            }
            _ => (a.dist(b), "px".into()),
        }
    }
}

/// Angle at vertex `v` between rays to `a` and `b`, in degrees (pixel space).
pub fn angle_deg(a: Pt, v: Pt, b: Pt) -> f64 {
    let u = a.sub(v);
    let w = b.sub(v);
    let d = (u.cross(w)).atan2(u.dot(w)).abs();
    d.to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xy(points: [Pt; 4], p: XyParams) -> Calibration {
        Axes { kind: AxesKind::Xy, points: points.to_vec(), params: AxesParams::Xy(p), image_height: 600.0 }
            .calibrate()
            .unwrap()
    }

    #[test]
    fn xy_linear_roundtrip() {
        // x: px 100 -> 0, px 700 -> 6 ; y: py 500 -> -1, py 100 -> 1
        let c = xy(
            [Pt::new(100.0, 500.0), Pt::new(700.0, 500.0), Pt::new(100.0, 500.0), Pt::new(100.0, 100.0)],
            XyParams { x1: "0".into(), x2: "6".into(), y1: "-1".into(), y2: "1".into(), ..Default::default() },
        );
        let d = c.to_data(Pt::new(500.0, 300.0));
        assert!((d[0] - 4.0).abs() < 1e-9 && d[1].abs() < 1e-9, "{d:?}");
        let p = c.to_pixel(&[4.0, 0.0]).unwrap();
        assert!(p.dist(Pt::new(500.0, 300.0)) < 1e-9);
    }

    #[test]
    fn xy_rotated_axes() {
        // axes rotated by 30°: build pixel points from a known transform.
        let th = 30f64.to_radians();
        let f = |x: f64, y: f64| Pt::new(200.0 + 50.0 * (x * th.cos() + y * th.sin()), 400.0 - 50.0 * (-x * th.sin() + y * th.cos()));
        let c = xy(
            [f(1.0, 0.0), f(5.0, 0.0), f(0.0, 2.0), f(0.0, 6.0)],
            XyParams { x1: "1".into(), x2: "5".into(), y1: "2".into(), y2: "6".into(), ..Default::default() },
        );
        let d = c.to_data(f(3.3, 4.4));
        assert!((d[0] - 3.3).abs() < 1e-9 && (d[1] - 4.4).abs() < 1e-9, "{d:?}");
    }

    #[test]
    fn xy_log_and_date() {
        let c = xy(
            [Pt::new(0.0, 100.0), Pt::new(300.0, 100.0), Pt::new(0.0, 100.0), Pt::new(0.0, 0.0)],
            XyParams { x1: "2020/01/01".into(), x2: "2020/01/31".into(), y1: "1".into(), y2: "1000".into(), log_y: true, ..Default::default() },
        );
        let d = c.to_data(Pt::new(150.0, 100.0 - 100.0 / 3.0));
        assert!((d[1] - 10.0).abs() < 1e-9);
        assert!(c.is_date(0) && !c.is_date(1));
        assert_eq!(c.format_value(0, d[0], NumFormat::Ignore, 3), "2020/01/16");
    }

    #[test]
    fn polar_roundtrip_ccw_and_cw() {
        for cw in [false, true] {
            let o = Pt::new(300.0, 300.0);
            // r scale: 50 px per unit, theta measured from +x
            let pt = |r: f64, t: f64| {
                let a = if cw { -t.to_radians() } else { t.to_radians() };
                Pt::new(o.x + 50.0 * r * a.cos(), o.y - 50.0 * r * a.sin())
            };
            let c = Axes {
                kind: AxesKind::Polar,
                points: vec![o, pt(2.0, 30.0), pt(3.0, 60.0)],
                params: AxesParams::Polar(PolarParams {
                    r1: "2".into(),
                    theta1: "30".into(),
                    r2: "3".into(),
                    theta2: "60".into(),
                    degrees: true,
                    clockwise: cw,
                    log_r: false,
                }),
                image_height: 600.0,
            }
            .calibrate()
            .unwrap();
            let d = c.to_data(pt(1.5, 200.0));
            assert!((d[0] - 1.5).abs() < 1e-9 && (d[1] - 200.0).abs() < 1e-9, "{d:?}");
            let p = c.to_pixel(&[1.5, 200.0]).unwrap();
            assert!(p.dist(pt(1.5, 200.0)) < 1e-9);
        }
    }

    #[test]
    fn ternary_corners() {
        let (a, b, cc) = (Pt::new(0.0, 100.0), Pt::new(100.0, 100.0), Pt::new(50.0, 13.4));
        let c = Axes {
            kind: AxesKind::Ternary,
            points: vec![a, b, cc],
            params: AxesParams::Ternary(TernaryParams { range100: true, normal: true }),
            image_height: 200.0,
        }
        .calibrate()
        .unwrap();
        let d = c.to_data(a);
        assert!((d[0] - 100.0).abs() < 1e-9 && d[1].abs() < 1e-9 && d[2].abs() < 1e-9);
        let centroid = Pt::new((a.x + b.x + cc.x) / 3.0, (a.y + b.y + cc.y) / 3.0);
        let d = c.to_data(centroid);
        assert!(d.iter().all(|v| (v - 100.0 / 3.0).abs() < 1e-6));
        let p = c.to_pixel(&[20.0, 30.0, 50.0]).unwrap();
        let back = c.to_data(p);
        assert!((back[0] - 20.0).abs() < 1e-9 && (back[2] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn map_scale() {
        let c = Axes {
            kind: AxesKind::Map,
            points: vec![Pt::new(10.0, 50.0), Pt::new(110.0, 50.0)],
            params: AxesParams::Map(MapParams { length: "200".into(), units: "mi".into() }),
            image_height: 100.0,
        }
        .calibrate()
        .unwrap();
        let (d, u) = c.distance(Pt::new(0.0, 0.0), Pt::new(30.0, 40.0));
        assert!((d - 100.0).abs() < 1e-9);
        assert_eq!(u, "mi");
        assert_eq!(c.to_data(Pt::new(50.0, 100.0)), vec![100.0, 0.0]);
    }

    #[test]
    fn bar_values() {
        let c = Axes {
            kind: AxesKind::Bar,
            points: vec![Pt::new(50.0, 400.0), Pt::new(50.0, 100.0)],
            params: AxesParams::Bar(BarParams { p1: "0".into(), p2: "30".into(), log: false }),
            image_height: 500.0,
        }
        .calibrate()
        .unwrap();
        assert_eq!(c.to_data(Pt::new(300.0, 250.0)), vec![15.0]);
        if let Calibration::Bar(b) = c {
            assert!(b.vertical_bars());
        }
    }

    #[test]
    fn angle() {
        assert!((angle_deg(Pt::new(1.0, 0.0), Pt::new(0.0, 0.0), Pt::new(0.0, 1.0)) - 90.0).abs() < 1e-9);
    }
}
