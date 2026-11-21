//! Automatic point extraction from a binary "selected pixels" map
//! (WebPlotDigitizer's algorithms plus bar extraction).

use plot2csv_core::axes::Calibration;
use plot2csv_core::dataset::{Algorithm, DataPoint};
use plot2csv_core::geom::Pt;

use crate::raster::{component_stats, label_components};

/// Selected pixels of an image.
pub struct Bitmap<'a> {
    pub bits: &'a [bool],
    pub width: usize,
    pub height: usize,
}

impl Bitmap<'_> {
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.width + x]
    }
}

pub fn run(alg: &Algorithm, bm: &Bitmap, cal: Option<&Calibration>) -> Result<Vec<DataPoint>, String> {
    match *alg {
        Algorithm::AveragingWindow { dx, dy } => Ok(averaging_window(bm, dx.max(1.0), dy.max(1.0))),
        Algorithm::XStepInterp { xmin, xstep, xmax, ymin, ymax, smoothing } => {
            x_step(bm, need_xy(cal)?, xmin, xstep, xmax, ymin, ymax, Some(smoothing))
        }
        Algorithm::XStep { xmin, xstep, xmax, ymin, ymax } => x_step(bm, need_xy(cal)?, xmin, xstep, xmax, ymin, ymax, None),
        Algorithm::Blob { min_d, max_d } => Ok(blobs(bm, min_d, max_d)),
        Algorithm::Bar { min_width } => bars(bm, cal, min_width),
        Algorithm::MultiTrack { .. } => Ok(averaging_window(bm, 10.0, 10.0)),
    }
}

fn need_xy(cal: Option<&Calibration>) -> Result<&Calibration, String> {
    match cal {
        Some(c @ Calibration::Xy(_)) => Ok(c),
        _ => Err("this algorithm needs a calibrated X-Y plot".into()),
    }
}

/// Column-wise blob centres, then merge candidates within a ΔX × ΔY window.
pub fn averaging_window(bm: &Bitmap, dx: f64, dy: f64) -> Vec<DataPoint> {
    let mut cand: Vec<(f64, f64)> = Vec::new();
    for x in 0..bm.width {
        let mut run_start: Option<usize> = None;
        let mut sum = 0.0;
        let mut n = 0.0;
        let mut last_y = 0usize;
        for y in 0..bm.height {
            if bm.get(x, y) {
                match run_start {
                    Some(_) if (y as f64) <= last_y as f64 + dy => {
                        sum += y as f64;
                        n += 1.0;
                    }
                    Some(_) => {
                        cand.push((x as f64 + 0.5, sum / n + 0.5));
                        run_start = Some(y);
                        sum = y as f64;
                        n = 1.0;
                    }
                    None => {
                        run_start = Some(y);
                        sum = y as f64;
                        n = 1.0;
                    }
                }
                last_y = y;
            }
        }
        if run_start.is_some() {
            cand.push((x as f64 + 0.5, sum / n + 0.5));
        }
    }
    // Merge: candidates are in x order; take the first unused, average all
    // unused candidates inside the window around it.
    let mut used = vec![false; cand.len()];
    let mut out = Vec::new();
    for i in 0..cand.len() {
        if used[i] {
            continue;
        }
        let (xi, yi) = cand[i];
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for j in i..cand.len() {
            let (xj, yj) = cand[j];
            if xj - xi > dx {
                break;
            }
            if !used[j] && (yj - yi).abs() <= dy {
                used[j] = true;
                sx += xj;
                sy += yj;
                n += 1.0;
            }
        }
        out.push(DataPoint::new(sx / n, sy / n));
    }
    out
}

/// X step (optionally with smoothing + cubic-spline interpolation), in data units.
#[allow(clippy::too_many_arguments)]
pub fn x_step(
    bm: &Bitmap,
    cal: &Calibration,
    xmin: f64,
    xstep: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
    smoothing: Option<f64>,
) -> Result<Vec<DataPoint>, String> {
    if xstep <= 0.0 || xmax <= xmin {
        return Err("X step must be positive and X_max > X_min".into());
    }
    let (ylo, yhi) = (ymin.min(ymax), ymin.max(ymax));
    // Data-space samples of selected pixels, grouped by pixel column.
    let mut samples: Vec<(f64, f64)> = Vec::new();
    for x in 0..bm.width {
        let (mut s, mut n, mut sx) = (0.0, 0.0, 0.0);
        for y in 0..bm.height {
            if bm.get(x, y) {
                let d = cal.to_data(Pt::new(x as f64 + 0.5, y as f64 + 0.5));
                if d[1] >= ylo && d[1] <= yhi {
                    s += d[1];
                    sx += d[0];
                    n += 1.0;
                }
            }
        }
        if n > 0.0 {
            samples.push((sx / n, s / n));
        }
    }
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let nsteps = ((xmax - xmin) / xstep).floor() as usize;
    match smoothing {
        None => {
            for k in 0..=nsteps {
                let xc = xmin + k as f64 * xstep;
                // the nearest column sample within half a step
                let i = samples.partition_point(|s| s.0 < xc);
                let mut best: Option<(f64, f64)> = None;
                for j in [i.saturating_sub(1), i] {
                    if let Some(s) = samples.get(j) {
                        if (s.0 - xc).abs() <= xstep / 2.0 && best.map(|b| (b.0 - xc).abs() > (s.0 - xc).abs()).unwrap_or(true) {
                            best = Some(*s);
                        }
                    }
                }
                if let Some((_, y)) = best {
                    if let Some(p) = cal.to_pixel(&[xc, y]) {
                        out.push(DataPoint::new(p.x, p.y));
                    }
                }
            }
        }
        Some(sm) => {
            let win = (sm / 100.0 * xstep).max(0.0);
            // Smooth: average samples within ±win of each sample.
            let smoothed: Vec<(f64, f64)> = if win > 0.0 {
                samples
                    .iter()
                    .map(|&(x, _)| {
                        let (mut s, mut n) = (0.0, 0.0);
                        let a = samples.partition_point(|q| q.0 < x - win);
                        for q in &samples[a..] {
                            if q.0 > x + win {
                                break;
                            }
                            s += q.1;
                            n += 1.0;
                        }
                        (x, s / n)
                    })
                    .collect()
            } else {
                samples.clone()
            };
            let mut xs: Vec<f64> = Vec::new();
            let mut ys: Vec<f64> = Vec::new();
            for (x, y) in smoothed {
                if xs.last().map(|l| x - l > 1e-12).unwrap_or(true) {
                    xs.push(x);
                    ys.push(y);
                }
            }
            let spline = CubicSpline::new(&xs, &ys);
            let (lo, hi) = (xs[0], *xs.last().unwrap());
            for k in 0..=nsteps {
                let xc = xmin + k as f64 * xstep;
                if xc < lo || xc > hi {
                    continue;
                }
                let y = spline.eval(xc);
                if let Some(p) = cal.to_pixel(&[xc, y]) {
                    out.push(DataPoint::new(p.x, p.y));
                }
            }
        }
    }
    Ok(out)
}

/// Natural cubic spline.
pub struct CubicSpline {
    x: Vec<f64>,
    a: Vec<f64>,
    b: Vec<f64>,
    c: Vec<f64>,
    d: Vec<f64>,
}

impl CubicSpline {
    pub fn new(x: &[f64], y: &[f64]) -> Self {
        let n = x.len();
        if n < 3 {
            // Linear fallback.
            let (b, slope) = if n == 2 { (vec![(y[1] - y[0]) / (x[1] - x[0])], 0.0) } else { (vec![0.0], 0.0) };
            let _ = slope;
            return Self { x: x.to_vec(), a: y.to_vec(), b, c: vec![0.0; n.max(1)], d: vec![0.0; n.max(1)] };
        }
        let h: Vec<f64> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
        let mut alpha = vec![0.0; n];
        for i in 1..n - 1 {
            alpha[i] = 3.0 / h[i] * (y[i + 1] - y[i]) - 3.0 / h[i - 1] * (y[i] - y[i - 1]);
        }
        let mut l = vec![1.0; n];
        let mut mu = vec![0.0; n];
        let mut z = vec![0.0; n];
        for i in 1..n - 1 {
            l[i] = 2.0 * (x[i + 1] - x[i - 1]) - h[i - 1] * mu[i - 1];
            mu[i] = h[i] / l[i];
            z[i] = (alpha[i] - h[i - 1] * z[i - 1]) / l[i];
        }
        let mut c = vec![0.0; n];
        let mut b = vec![0.0; n - 1];
        let mut d = vec![0.0; n - 1];
        for j in (0..n - 1).rev() {
            c[j] = z[j] - mu[j] * c[j + 1];
            b[j] = (y[j + 1] - y[j]) / h[j] - h[j] * (c[j + 1] + 2.0 * c[j]) / 3.0;
            d[j] = (c[j + 1] - c[j]) / (3.0 * h[j]);
        }
        Self { x: x.to_vec(), a: y.to_vec(), b, c, d }
    }

    pub fn eval(&self, x: f64) -> f64 {
        let n = self.x.len();
        if n == 1 {
            return self.a[0];
        }
        if self.d.len() < n - 1 || n == 2 {
            return self.a[0] + self.b[0] * (x - self.x[0]);
        }
        let i = (self.x.partition_point(|v| *v <= x).max(1) - 1).min(n - 2);
        let t = x - self.x[i];
        self.a[i] + self.b[i] * t + self.c[i] * t * t + self.d[i] * t * t * t
    }
}

/// Centroids of connected blobs whose equivalent diameter is within range.
pub fn blobs(bm: &Bitmap, min_d: f64, max_d: f64) -> Vec<DataPoint> {
    let (labels, n) = label_components(bm.bits, bm.width, bm.height, true);
    let stats = component_stats(&labels, n, bm.width);
    let mut out = Vec::new();
    for s in stats.iter().skip(1) {
        if s.area == 0 {
            continue;
        }
        let d = 2.0 * (s.area as f64 / std::f64::consts::PI).sqrt();
        if d >= min_d && d <= max_d {
            let (cx, cy) = s.centroid();
            out.push(DataPoint::new(cx + 0.5, cy + 0.5));
        }
    }
    out.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
    out
}

/// Bars: group adjacent columns (or rows) with selected pixels; each group gives
/// one point at the bar end farthest from the zero line.
pub fn bars(bm: &Bitmap, cal: Option<&Calibration>, min_width: f64) -> Result<Vec<DataPoint>, String> {
    let (vertical, base) = match cal {
        Some(Calibration::Bar(b)) => (b.vertical_bars(), b.axis_point(0.0).or(b.axis_point(1e-300))),
        Some(Calibration::Xy(_)) | None | Some(_) => (true, None),
    };
    let (len_a, len_b) = if vertical { (bm.width, bm.height) } else { (bm.height, bm.width) };
    let get = |a: usize, b: usize| if vertical { bm.get(a, b) } else { bm.get(b, a) };
    // For each column: extent of selected pixels.
    let ext: Vec<Option<(usize, usize, usize)>> = (0..len_a)
        .map(|a| {
            let mut lo = usize::MAX;
            let mut hi = 0;
            let mut n = 0;
            for b in 0..len_b {
                if get(a, b) {
                    lo = lo.min(b);
                    hi = hi.max(b);
                    n += 1;
                }
            }
            (n > 0).then_some((lo, hi, n))
        })
        .collect();
    let mut out = Vec::new();
    let mut a = 0;
    while a < len_a {
        if ext[a].is_none() {
            a += 1;
            continue;
        }
        let start = a;
        let mut los = Vec::new();
        let mut his = Vec::new();
        while a < len_a {
            match ext[a] {
                Some((lo, hi, _)) => {
                    los.push(lo);
                    his.push(hi);
                    a += 1;
                }
                None => break,
            }
        }
        if ((a - start) as f64) < min_width {
            continue;
        }
        los.sort_unstable();
        his.sort_unstable();
        let lo = los[los.len() / 2] as f64;
        let hi = his[his.len() / 2] as f64 + 1.0;
        let center = (start + a) as f64 / 2.0;
        // Choose the end farther from the baseline (handles negative bars).
        let base_b = base.map(|p| if vertical { p.y } else { p.x });
        let end = match base_b {
            Some(bb) => {
                if (lo - bb).abs() > (hi - bb).abs() {
                    lo
                } else {
                    hi
                }
            }
            None => {
                if vertical {
                    lo
                } else {
                    hi
                }
            }
        };
        let p = if vertical { Pt::new(center, end) } else { Pt::new(end, center) };
        out.push(DataPoint::new(p.x, p.y));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plot2csv_core::axes::{Axes, AxesKind, AxesParams, XyParams};

    fn bitmap_line(w: usize, h: usize, f: impl Fn(usize) -> usize) -> Vec<bool> {
        let mut b = vec![false; w * h];
        for x in 0..w {
            let y = f(x);
            for t in 0..3 {
                b[(y + t).min(h - 1) * w + x] = true;
            }
        }
        b
    }

    #[test]
    fn averaging_window_follows_line() {
        let (w, h) = (100, 50);
        let bits = bitmap_line(w, h, |x| 10 + x / 5);
        let pts = averaging_window(&Bitmap { bits: &bits, width: w, height: h }, 10.0, 10.0);
        assert!(pts.len() >= 8 && pts.len() <= 11, "{}", pts.len());
        for p in &pts {
            let expect = 10.0 + (p.x - 0.5) / 5.0 + 1.5;
            assert!((p.y - expect).abs() < 2.0, "{p:?} {expect}");
        }
    }

    #[test]
    fn x_step_interp_on_xy() {
        let (w, h) = (200, 100);
        let bits = bitmap_line(w, h, |x| 80 - x / 4);
        let mut axes = Axes::new(AxesKind::Xy, h as f64);
        axes.points = vec![Pt::new(0.0, 100.0), Pt::new(200.0, 100.0), Pt::new(0.0, 100.0), Pt::new(0.0, 0.0)];
        axes.params = AxesParams::Xy(XyParams { x1: "0".into(), x2: "20".into(), y1: "0".into(), y2: "10".into(), ..Default::default() });
        let cal = axes.calibrate().unwrap();
        let pts = x_step(&Bitmap { bits: &bits, width: w, height: h }, &cal, 1.0, 1.0, 19.0, 0.0, 10.0, Some(0.0)).unwrap();
        assert_eq!(pts.len(), 19);
        for p in &pts {
            let d = cal.to_data(p.pt());
            // y(px) = 80 - x/4 + 1 (thickness centre) -> data y = (100 - py)/10
            let expect = (100.0 - (80.0 - (d[0] * 10.0) / 4.0 + 1.5)) / 10.0;
            assert!((d[1] - expect).abs() < 0.1, "{d:?} {expect}");
        }
    }

    #[test]
    fn blob_centroids() {
        let (w, h) = (40, 20);
        let mut bits = vec![false; w * h];
        for (cx, cy) in [(10usize, 10usize), (30, 8)] {
            for y in cy - 2..=cy + 2 {
                for x in cx - 2..=cx + 2 {
                    bits[y * w + x] = true;
                }
            }
        }
        let pts = blobs(&Bitmap { bits: &bits, width: w, height: h }, 2.0, 20.0);
        assert_eq!(pts.len(), 2);
        assert!((pts[0].x - 10.5).abs() < 1e-9 && (pts[0].y - 10.5).abs() < 1e-9);
    }

    #[test]
    fn bar_tops() {
        let (w, h) = (60, 50);
        let mut bits = vec![false; w * h];
        for (x0, top) in [(5usize, 20usize), (25, 10), (45, 30)] {
            for x in x0..x0 + 8 {
                for y in top..40 {
                    bits[y * w + x] = true;
                }
            }
        }
        let pts = bars(&Bitmap { bits: &bits, width: w, height: h }, None, 2.0).unwrap();
        assert_eq!(pts.len(), 3);
        assert_eq!((pts[1].x, pts[1].y), (29.0, 10.0));
    }

    #[test]
    fn spline_reproduces_cubic_like_data() {
        let xs: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| x.sin()).collect();
        let s = CubicSpline::new(&xs, &ys);
        assert!((s.eval(4.5) - 4.5f64.sin()).abs() < 0.02);
    }
}
