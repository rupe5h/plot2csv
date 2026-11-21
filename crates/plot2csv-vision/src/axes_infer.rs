//! Automatic axis calibration from tick marks / grid lines and OCR'd tick labels.

use image::GrayImage;
use plot2csv_core::axes::{Axes, AxesKind, AxesParams, BarParams, XyParams};
use plot2csv_core::geom::{Pt, RectF};
use plot2csv_core::project::{AxisEvidence, Panel, TextItem, TextRole};
use plot2csv_core::value::{format_date, parse_date};

use crate::lines::integral;

pub struct AxesResult {
    pub axes: Axes,
    pub evidence: AxisEvidence,
    pub note: String,
    /// 0..1
    pub confidence: f64,
}

/// Result of fitting one axis.
#[derive(Clone, Debug)]
pub struct AxisFit {
    /// Preference when fits are otherwise equal (e.g. superscript reading of "10k" labels).
    pub prior: i32,
    pub log: bool,
    pub date: bool,
    /// value (or log10 value) = a + b * px
    pub a: f64,
    pub b: f64,
    /// (px, value, text index) for inliers
    pub inliers: Vec<(f64, f64, usize)>,
    pub outliers: Vec<usize>,
    pub rms_px: f64,
}

impl AxisFit {
    pub fn px_of(&self, v: f64) -> Option<f64> {
        let t = if self.log { (v > 0.0).then(|| v.log10())? } else { v };
        Some((t - self.a) / self.b)
    }
}

/// Darkness profile helpers on a grey image.
struct Gray<'a> {
    g: &'a GrayImage,
    integ: Vec<u64>,
}

impl<'a> Gray<'a> {
    fn new(g: &'a GrayImage) -> Self {
        Self { g, integ: integral(g) }
    }
    fn w(&self) -> i64 {
        self.g.width() as i64
    }
    fn h(&self) -> i64 {
        self.g.height() as i64
    }
    fn dark(&self, x: i64, y: i64, thr: u8) -> bool {
        x >= 0 && y >= 0 && x < self.w() && y < self.h() && self.g.get_pixel(x as u32, y as u32).0[0] < thr
    }
    fn mean(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> f64 {
        crate::lines::box_mean(&self.integ, self.g.width() as usize, self.g.height() as usize, x0, y0, x1, y1)
    }
}

/// Refine an axis line position: the row (or column) near `guess` that is darkest
/// along the span.
fn refine_line(g: &Gray, horizontal: bool, guess: f64, span: (f64, f64), search: i64) -> f64 {
    let mut best = (guess, f64::MAX);
    for d in -search..=search {
        let c = guess.round() as i64 + d;
        let m = if horizontal {
            g.mean(span.0 as i64, c, span.1 as i64, c + 1)
        } else {
            g.mean(c, span.0 as i64, c + 1, span.1 as i64)
        };
        if m < best.1 {
            best = (c as f64 + 0.5, m);
        }
    }
    best.0
}

/// Tick marks along an axis line and grid lines across the plot.
fn find_ticks(g: &Gray, inner: &RectF, horizontal: bool, axis_pos: f64) -> Vec<f64> {
    let thr = 150u8;
    let (lo, hi) = if horizontal { (inner.x0, inner.x1) } else { (inner.y0, inner.y1) };
    let len_cross = if horizontal { inner.height() } else { inner.width() };
    let tick_len = (0.012 * len_cross).clamp(3.0, 12.0) as i64;
    let a = axis_pos.round() as i64;
    let mut hits: Vec<i64> = Vec::new();
    for t in (lo.round() as i64 + 2)..(hi.round() as i64 - 1) {
        // Fraction of dark pixels in the window outside and inside the axis.
        let mut out_n = 0;
        let mut in_n = 0;
        for k in 2..2 + tick_len {
            let (xo, yo, xi, yi) = if horizontal { (t, a + k, t, a - k) } else { (a - k, t, a + k, t) };
            if g.dark(xo, yo, thr) {
                out_n += 1;
            }
            if g.dark(xi, yi, thr) {
                in_n += 1;
            }
        }
        let frac = out_n.max(in_n) as f64 / tick_len as f64;
        // Grid line: long run of slightly darker pixels across the plot.
        let grid = if horizontal {
            let m = g.mean(t, inner.y0 as i64 + 3, t + 1, inner.y1 as i64 - 3);
            let bg = g.mean(t + 3, inner.y0 as i64 + 3, t + 6, inner.y1 as i64 - 3).max(g.mean(t - 6, inner.y0 as i64 + 3, t - 3, inner.y1 as i64 - 3));
            bg - m > 12.0
        } else {
            let m = g.mean(inner.x0 as i64 + 3, t, inner.x1 as i64 - 3, t + 1);
            let bg = g.mean(inner.x0 as i64 + 3, t + 3, inner.x1 as i64 - 3, t + 6).max(g.mean(inner.x0 as i64 + 3, t - 6, inner.x1 as i64 - 3, t - 3));
            bg - m > 12.0
        };
        if frac >= 0.75 || grid {
            hits.push(t);
        }
    }
    // Merge adjacent hits into centres; drop very wide groups (data lines, bars).
    let mut out = Vec::new();
    let mut i = 0;
    while i < hits.len() {
        let mut j = i;
        while j + 1 < hits.len() && hits[j + 1] - hits[j] <= 1 {
            j += 1;
        }
        if hits[j] - hits[i] <= 6 {
            out.push((hits[i] + hits[j]) as f64 / 2.0 + 0.5);
        }
        i = j + 1;
    }
    out
}

/// Alternative reading of labels like "102" as 10², "10-3" as 10⁻³, "10°" as 10⁰.
fn superscript_value(text: &str) -> Option<f64> {
    let t = plot2csv_core::value::normalize_number_text(text).replace(' ', "");
    let rest = t.strip_prefix("10")?;
    if rest.is_empty() {
        return Some(10.0);
    }
    match rest {
        "°" | "o" | "O" => return Some(1.0),
        "'" | "’" | "`" | "|" | "i" | "l" => return Some(10.0),
        _ => {}
    }
    // Explicit exponent ("10^2", "10^-1"), e.g. from the exponent OCR pass.
    let rest = rest.strip_prefix('^').unwrap_or(rest);
    let k: i32 = rest.parse().ok()?;
    (k.abs() <= 30).then(|| 10f64.powi(k))
}

/// Fit value = a + b*px (or log10) to (px, value, idx) samples by exhaustive-pair RANSAC.
fn ransac(samples: &[(f64, f64, usize)], log: bool, tol_px: f64) -> Option<AxisFit> {
    let pts: Vec<(f64, f64, usize)> = samples
        .iter()
        .filter_map(|&(p, v, i)| {
            if log {
                (v > 0.0).then(|| (p, v.log10(), i))
            } else {
                Some((p, v, i))
            }
        })
        .collect();
    if pts.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, f64, f64, f64)> = None; // (inliers, err, a, b)
    for i in 0..pts.len() {
        for j in i + 1..pts.len() {
            let (p1, v1, _) = pts[i];
            let (p2, v2, _) = pts[j];
            if (p2 - p1).abs() < 3.0 || v1 == v2 {
                continue;
            }
            let b = (v2 - v1) / (p2 - p1);
            let a = v1 - b * p1;
            let mut n = 0;
            let mut err = 0.0;
            for &(p, v, _) in &pts {
                let e = ((v - a) / b - p).abs();
                if e <= tol_px {
                    n += 1;
                    err += e;
                }
            }
            let better = match best {
                None => true,
                Some((bn, be, _, _)) => n > bn || (n == bn && err < be),
            };
            if better {
                best = Some((n, err, a, b));
            }
        }
    }
    let (n, _, a0, b0) = best?;
    if n < 2 {
        return None;
    }
    // Least-squares refit on inliers (value as a function of px).
    let inl: Vec<(f64, f64, usize)> = pts.iter().copied().filter(|&(p, v, _)| ((v - a0) / b0 - p).abs() <= tol_px).collect();
    let (a, b) = if inl.len() >= 3 {
        let n = inl.len() as f64;
        let mp = inl.iter().map(|x| x.0).sum::<f64>() / n;
        let mv = inl.iter().map(|x| x.1).sum::<f64>() / n;
        let sxx: f64 = inl.iter().map(|x| (x.0 - mp).powi(2)).sum();
        let sxy: f64 = inl.iter().map(|x| (x.0 - mp) * (x.1 - mv)).sum();
        if sxx > 1e-9 {
            let b = sxy / sxx;
            (mv - b * mp, b)
        } else {
            (a0, b0)
        }
    } else {
        (a0, b0)
    };
    let rms = (inl.iter().map(|&(p, v, _)| ((v - a) / b - p).powi(2)).sum::<f64>() / inl.len() as f64).sqrt();
    let inlier_idx: Vec<usize> = inl.iter().map(|x| x.2).collect();
    let outliers = samples.iter().map(|s| s.2).filter(|i| !inlier_idx.contains(i)).collect();
    // Report the label values themselves (10^log10(500) is 499.99999999999994).
    let original = |i: usize, v: f64| samples.iter().find(|s| s.2 == i).map(|s| s.1).unwrap_or(if log { 10f64.powf(v) } else { v });
    let inliers = inl.iter().map(|&(p, v, i)| (p, original(i, v), i)).collect();
    Some(AxisFit { prior: 0, log, date: false, a, b, inliers, outliers, rms_px: rms })
}

/// Fit one axis from its tick labels: tries linear, log and superscript-repaired log.
pub fn fit_axis(items: &[TextItem], role: TextRole, ticks: &[f64], horizontal: bool, axis_len: f64) -> Option<AxisFit> {
    let tol_match = (0.03 * axis_len).max(6.0);
    let tol_fit = (0.012 * axis_len).max(3.0);
    let anchor = |it: &TextItem| if horizontal { it.bbox.center().x } else { it.bbox.center().y };
    let snap = |p: f64, w: f64| -> f64 {
        ticks
            .iter()
            .copied()
            .filter(|t| (t - p).abs() <= tol_match.max(0.6 * w))
            .min_by(|a, b| (a - p).abs().partial_cmp(&(b - p).abs()).unwrap())
            .unwrap_or(p)
    };
    let labelled: Vec<(usize, &TextItem)> = items.iter().enumerate().filter(|(_, it)| it.role == role).collect();
    if labelled.len() < 2 {
        return None;
    }
    let size = |it: &TextItem| if horizontal { it.bbox.width() } else { it.bbox.height() };
    let is_date = labelled.iter().filter(|(_, it)| parse_date(&it.text).is_some()).count() >= 2;
    let plain: Vec<(f64, f64, usize)> = labelled
        .iter()
        .filter_map(|(i, it)| {
            let v = if is_date { parse_date(&it.text)? } else { it.value? };
            Some((snap(anchor(it), size(it)), v, *i))
        })
        .collect();
    let sup: Vec<(f64, f64, usize)> = labelled
        .iter()
        .filter_map(|(i, it)| {
            let v = superscript_value(&it.text).or(it.value)?;
            Some((snap(anchor(it), size(it)), v, *i))
        })
        .collect();
    let mut cands: Vec<AxisFit> = Vec::new();
    if let Some(mut f) = ransac(&plain, false, tol_fit) {
        f.date = is_date;
        cands.push(f);
    }
    if !is_date {
        let span = |s: &[(f64, f64, usize)]| {
            let pos: Vec<f64> = s.iter().map(|x| x.1).filter(|v| *v > 0.0).collect();
            let mx = pos.iter().cloned().fold(f64::MIN, f64::max);
            let mn = pos.iter().cloned().fold(f64::MAX, f64::min);
            if pos.len() >= 2 { mx / mn } else { 1.0 }
        };
        if span(&plain) >= 20.0 {
            if let Some(f) = ransac(&plain, true, tol_fit) {
                cands.push(f);
            }
        }
        if sup.iter().zip(&plain).any(|(a, b)| a.1 != b.1) || sup.len() != plain.len() {
            if let Some(mut f) = ransac(&sup, true, tol_fit) {
                // All inlier labels look like "10", "10k", "10-k": a power-of-ten axis.
                let all_pow = f.inliers.iter().all(|&(_, _, i)| {
                    let t = plot2csv_core::value::normalize_number_text(&items[i].text).replace(' ', "");
                    t.starts_with("10") && t[2..].trim_start_matches('-').chars().all(|c| c.is_ascii_digit())
                });
                if all_pow {
                    f.prior = 1;
                }
                cands.push(f);
            }
        }
    }
    // Decade sequence: labels that all look like 10^k (exponents unreadable) and
    // are evenly spaced -> consecutive powers of ten, increasing right / up.
    let pow_like: Vec<&(usize, &TextItem)> = labelled
        .iter()
        .filter(|(_, it)| {
            let t = plot2csv_core::value::normalize_number_text(&it.text).replace(' ', "");
            t.starts_with("10") && t.len() <= 5 && t.chars().count() <= 4
        })
        .collect();
    if pow_like.len() >= 3 && pow_like.len() * 2 >= labelled.len() {
        let mut pos: Vec<(f64, usize, &TextItem)> = pow_like.iter().map(|(i, it)| (snap(anchor(it), size(it)), *i, *it)).collect();
        // Increasing value direction: +x for horizontal axes, -y for vertical.
        pos.sort_by(|a, b| if horizontal { a.0.partial_cmp(&b.0).unwrap() } else { b.0.partial_cmp(&a.0).unwrap() });
        let gaps: Vec<f64> = pos.windows(2).map(|w| (w[1].0 - w[0].0).abs()).collect();
        let mg = gaps.iter().sum::<f64>() / gaps.len() as f64;
        if gaps.iter().all(|g| (g - mg).abs() < 0.15 * mg) {
            // Anchor the exponent with any label whose exponent was read.
            let offs: Vec<i32> = pos
                .iter()
                .enumerate()
                .filter_map(|(rank, (_, _, it))| superscript_value(&it.text).map(|v| v.log10().round() as i32 - rank as i32))
                .collect();
            let mut counts = std::collections::HashMap::new();
            for o in &offs {
                *counts.entry(*o).or_insert(0) += 1;
            }
            let base = counts.into_iter().max_by_key(|(o, c)| (*c, -o.abs())).map(|(o, _)| o).unwrap_or(0);
            let seq: Vec<(f64, f64, usize)> = pos.iter().enumerate().map(|(rank, (p, i, _))| (*p, 10f64.powi(base + rank as i32), *i)).collect();
            if let Some(mut f) = ransac(&seq, true, tol_fit) {
                f.prior = 2;
                cands.push(f);
            }
        }
    }
    // Most inliers, then lowest residual; a log fit must beat linear clearly.
    cands.sort_by(|x, y| {
        y.inliers
            .len()
            .cmp(&x.inliers.len())
            .then(y.prior.cmp(&x.prior))
            .then(x.rms_px.partial_cmp(&y.rms_px).unwrap())
    });
    cands.into_iter().next()
}

fn fmt_value(v: f64, date: bool) -> String {
    if date {
        format_date(v, "yyyy/mm/dd")
    } else {
        let s = format!("{}", v);
        if s.len() > 12 {
            format!("{v:e}")
        } else {
            s
        }
    }
}

/// Axis line positions and tick/grid positions of a panel:
/// (x-axis y, y-axis x, x ticks, y ticks).
pub fn axis_ticks(gray: &GrayImage, inner: &RectF) -> (f64, f64, Vec<f64>, Vec<f64>) {
    let g = Gray::new(gray);
    let x_axis_y = refine_line(&g, true, inner.y1, (inner.x0, inner.x1), 4);
    let y_axis_x = refine_line(&g, false, inner.x0, (inner.y0, inner.y1), 4);
    let xt = find_ticks(&g, inner, true, x_axis_y);
    let yt = find_ticks(&g, inner, false, y_axis_x);
    (x_axis_y, y_axis_x, xt, yt)
}

/// Infer the calibration of one panel.
pub fn infer_axes(gray: &GrayImage, panel: &Panel, items: &[TextItem]) -> AxesResult {
    let inner = panel.inner;
    let h_img = gray.height() as f64;
    let mut axes = Axes::new(panel.kind, h_img);
    let mut ev = AxisEvidence::default();
    if !panel.kind.auto_calibratable() {
        let note = format!("{}: place the calibration points by hand ({}).", panel.kind.label(), panel.kind.point_help());
        return AxesResult { axes, evidence: ev, note, confidence: 0.0 };
    }
    let g = Gray::new(gray);
    let x_axis_y = refine_line(&g, true, inner.y1, (inner.x0, inner.x1), 4);
    let y_axis_x = refine_line(&g, false, inner.x0, (inner.y0, inner.y1), 4);
    ev.x_axis = Some([Pt::new(inner.x0, x_axis_y), Pt::new(inner.x1, x_axis_y)]);
    ev.y_axis = Some([Pt::new(y_axis_x, inner.y0), Pt::new(y_axis_x, inner.y1)]);
    ev.x_ticks = find_ticks(&g, &inner, true, x_axis_y);
    ev.y_ticks = find_ticks(&g, &inner, false, y_axis_x);
    let fx = fit_axis(items, TextRole::XTick, &ev.x_ticks, true, inner.width());
    let fy = fit_axis(items, TextRole::YTick, &ev.y_ticks, false, inner.height());
    if let Some(f) = &fx {
        ev.x_matches = f.inliers.clone();
    }
    if let Some(f) = &fy {
        ev.y_matches = f.inliers.clone();
    }
    let extremes = |f: &AxisFit| -> Option<((f64, f64), (f64, f64))> {
        let mut v = f.inliers.clone();
        v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (a, b) = (v.first()?, v.last()?);
        // Use the fitted positions for the chosen values (less noise than single labels).
        let pa = f.px_of(a.1).unwrap_or(a.0);
        let pb = f.px_of(b.1).unwrap_or(b.0);
        Some(((pa, a.1), (pb, b.1)))
    };
    let mut notes = Vec::new();
    let mut conf = 0.0;
    match panel.kind {
        AxesKind::Bar => {
            // Value axis: the one with a fit (prefer Y).
            let (fit, horizontal) = match (&fy, &fx) {
                (Some(f), _) => (Some(f), false),
                (None, Some(f)) => (Some(f), true),
                _ => (None, false),
            };
            let mut p = BarParams::default();
            if let Some(f) = fit {
                if let Some(((p1, v1), (p2, v2))) = extremes(f) {
                    p.p1 = fmt_value(v1, false);
                    p.p2 = fmt_value(v2, false);
                    p.log = f.log;
                    axes.points = if horizontal {
                        vec![Pt::new(p1, x_axis_y), Pt::new(p2, x_axis_y)]
                    } else {
                        vec![Pt::new(y_axis_x, p1), Pt::new(y_axis_x, p2)]
                    };
                    conf = (f.inliers.len() as f64 / 5.0).min(1.0) * 0.9;
                    notes.push(format!("Value axis: {} labels fit ({}{}).", f.inliers.len(), if f.log { "log" } else { "linear" }, outlier_note(f)));
                }
            } else {
                notes.push("Could not read the value-axis labels; place P1 and P2 by hand.".into());
            }
            axes.params = AxesParams::Bar(p);
        }
        _ => {
            let mut p = XyParams::default();
            let mut pts = vec![Pt::default(); 4];
            let mut have = [false, false];
            if let Some(f) = &fx {
                if let Some(((p1, v1), (p2, v2))) = extremes(f) {
                    pts[0] = Pt::new(p1, x_axis_y);
                    pts[1] = Pt::new(p2, x_axis_y);
                    p.x1 = fmt_value(v1, f.date);
                    p.x2 = fmt_value(v2, f.date);
                    p.log_x = f.log;
                    have[0] = true;
                    notes.push(format!(
                        "X: {} labels fit a {} scale{}.",
                        f.inliers.len(),
                        if f.date { "date" } else if f.log { "log" } else { "linear" },
                        outlier_note(f)
                    ));
                }
            }
            if let Some(f) = &fy {
                if let Some(((p1, v1), (p2, v2))) = extremes(f) {
                    // Y1 is the lower value (bottom).
                    let ((pa, va), (pb, vb)) = if v1 <= v2 { ((p1, v1), (p2, v2)) } else { ((p2, v2), (p1, v1)) };
                    pts[2] = Pt::new(y_axis_x, pa);
                    pts[3] = Pt::new(y_axis_x, pb);
                    p.y1 = fmt_value(va, f.date);
                    p.y2 = fmt_value(vb, f.date);
                    p.log_y = f.log;
                    have[1] = true;
                    notes.push(format!(
                        "Y: {} labels fit a {} scale{}.",
                        f.inliers.len(),
                        if f.date { "date" } else if f.log { "log" } else { "linear" },
                        outlier_note(f)
                    ));
                }
            }
            if have[0] && have[1] {
                axes.points = pts;
                let n = fx.as_ref().map(|f| f.inliers.len()).unwrap_or(0) + fy.as_ref().map(|f| f.inliers.len()).unwrap_or(0);
                conf = (n as f64 / 8.0).min(1.0) * 0.95;
            } else {
                if !have[0] {
                    notes.push("X axis: not enough readable tick labels.".into());
                }
                if !have[1] {
                    notes.push("Y axis: not enough readable tick labels.".into());
                }
                notes.push("Place the missing calibration points by hand and type their values.".into());
                // Keep what was found so the user only places the missing ones.
                if have[0] {
                    axes.points = pts[..2].to_vec();
                }
            }
            axes.params = AxesParams::Xy(p);
        }
    }
    AxesResult { axes, evidence: ev, note: notes.join(" "), confidence: conf }
}

fn outlier_note(f: &AxisFit) -> String {
    if f.outliers.is_empty() {
        String::new()
    } else {
        format!(", {} label(s) ignored as misread", f.outliers.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn tick_item(x: f64, y: f64, text: &str, role: TextRole) -> TextItem {
        TextItem {
            bbox: RectF::new(x - 8.0, y - 6.0, x + 8.0, y + 6.0),
            text: text.into(),
            conf: 0.9,
            role,
            value: plot2csv_core::value::parse_number(text),
            manual: false,
            vertical: false,
        }
    }

    fn panel(inner: RectF) -> Panel {
        Panel { id: 1, name: "p".into(), kind: AxesKind::Xy, inner, outer: inner.expand(40.0), enabled: true }
    }

    #[test]
    fn linear_axes_from_labels() {
        let mut g = GrayImage::from_pixel(600, 400, Luma([255]));
        let inner = RectF::new(80.0, 40.0, 560.0, 330.0);
        for x in 80..=560 {
            g.put_pixel(x, 330, Luma([0]));
        }
        for y in 40..=330 {
            g.put_pixel(80, y, Luma([0]));
        }
        let mut items = Vec::new();
        for k in 0..=6 {
            let x = 80.0 + 80.0 * k as f64;
            for d in 1..6 {
                g.put_pixel(x as u32, 330 + d, Luma([0]));
            }
            items.push(tick_item(x, 345.0, &k.to_string(), TextRole::XTick));
        }
        // one misread label
        items.push(tick_item(80.0 + 80.0 * 3.0 + 1.0, 345.0, "8", TextRole::XTick));
        for (k, v) in ["-1", "-0.5", "0", "0.5", "1"].iter().enumerate() {
            let y = 330.0 - 72.5 * k as f64;
            items.push(tick_item(60.0, y, v, TextRole::YTick));
        }
        let r = infer_axes(&g, &panel(inner), &items);
        let cal = r.axes.calibrate().expect(&r.note);
        let d = cal.to_data(Pt::new(80.0 + 80.0 * 4.5, 330.0 - 72.5 * 3.0));
        assert!((d[0] - 4.5).abs() < 0.05 && (d[1] - 0.5).abs() < 0.02, "{d:?} {}", r.note);
        assert!(r.note.contains("ignored"), "{}", r.note);
    }

    #[test]
    fn log_axis_with_superscripts() {
        let g = GrayImage::from_pixel(600, 400, Luma([255]));
        let inner = RectF::new(80.0, 40.0, 560.0, 330.0);
        let mut items = Vec::new();
        for (k, t) in ["0", "2", "4", "6"].iter().enumerate() {
            items.push(tick_item(80.0 + 160.0 * k as f64, 345.0, t, TextRole::XTick));
        }
        // 10^0 .. 10^3 read as "100", "101", "102", "103"
        for k in 0..4 {
            items.push(tick_item(60.0, 330.0 - 96.0 * k as f64, &format!("10{k}"), TextRole::YTick));
        }
        let r = infer_axes(&g, &panel(inner), &items);
        let cal = r.axes.calibrate().expect(&r.note);
        let d = cal.to_data(Pt::new(200.0, 330.0 - 96.0 * 1.5));
        assert!((d[1] - 10f64.powf(1.5)).abs() < 0.5, "{d:?} {}", r.note);
        assert!(r.note.contains("log"), "{}", r.note);
    }

    #[test]
    fn log_axis_values_are_the_label_values() {
        let g = GrayImage::from_pixel(600, 400, Luma([255]));
        let inner = RectF::new(80.0, 40.0, 560.0, 330.0);
        let mut items = Vec::new();
        for (k, t) in ["5", "50", "500"].iter().enumerate() {
            items.push(tick_item(80.0 + 200.0 * k as f64, 345.0, t, TextRole::XTick));
        }
        for (k, t) in ["10^-1", "10^0", "10^1", "10^2"].iter().enumerate() {
            items.push(tick_item(60.0, 330.0 - 96.0 * k as f64, t, TextRole::YTick));
        }
        let r = infer_axes(&g, &panel(inner), &items);
        let AxesParams::Xy(p) = &r.axes.params else { panic!("{}", r.note) };
        assert_eq!((p.x1.as_str(), p.x2.as_str()), ("5", "500"), "{}", r.note);
        assert_eq!((p.y1.as_str(), p.y2.as_str()), ("0.1", "100"), "{}", r.note);
        assert!(p.log_x && p.log_y);
    }
}
