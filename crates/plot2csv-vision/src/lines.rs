//! Ink detection and straight-line extraction (fine-angle Hough + run verification).

use image::GrayImage;
use plot2csv_core::geom::Pt;

/// Binary "ink" map: pixels clearly darker than their local background.
/// Robust to uneven lighting (photos).
pub struct Ink {
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
}

impl Ink {
    #[inline]
    pub fn get(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.bits[y as usize * self.w + x as usize]
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }
}

/// Integral image of a grey image (u64 sums), size (w+1)×(h+1).
pub fn integral(g: &GrayImage) -> Vec<u64> {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let mut s = vec![0u64; (w + 1) * (h + 1)];
    let raw = g.as_raw();
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            row += raw[y * w + x] as u64;
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + row;
        }
    }
    s
}

/// Mean of a box using an integral image.
#[inline]
pub fn box_mean(s: &[u64], w: usize, h: usize, x0: i64, y0: i64, x1: i64, y1: i64) -> f64 {
    let x0 = x0.clamp(0, w as i64) as usize;
    let x1 = x1.clamp(0, w as i64) as usize;
    let y0 = y0.clamp(0, h as i64) as usize;
    let y1 = y1.clamp(0, h as i64) as usize;
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
    let sw = w + 1;
    (s[y1 * sw + x1] + s[y0 * sw + x0] - s[y0 * sw + x1] - s[y1 * sw + x0]) as f64 / n
}

pub fn detect_ink(g: &GrayImage, radius: i64, min_contrast: f64) -> Ink {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let s = integral(g);
    let raw = g.as_raw();
    let mut bits = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let m = box_mean(&s, w, h, x as i64 - radius, y as i64 - radius, x as i64 + radius + 1, y as i64 + radius + 1);
            let v = raw[y * w + x] as f64;
            bits[y * w + x] = m - v > min_contrast.max(0.12 * m) || v < 60.0;
        }
    }
    Ink { w, h, bits }
}

/// A verified straight segment.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: Pt,
    pub b: Pt,
    /// Normal angle of the line (radians) and distance from origin.
    pub phi: f64,
    pub rho: f64,
    pub votes: u32,
}

impl Segment {
    pub fn len(&self) -> f64 {
        self.a.dist(self.b)
    }
    pub fn is_empty(&self) -> bool {
        self.len() < 1e-9
    }
    /// Direction angle in degrees in (-90, 90].
    pub fn angle_deg(&self) -> f64 {
        let d = self.b.sub(self.a);
        let mut a = d.y.atan2(d.x).to_degrees();
        while a <= -90.0 {
            a += 180.0;
        }
        while a > 90.0 {
            a -= 180.0;
        }
        a
    }
    /// Homogeneous line (a, b, c) with a²+b²=1: a x + b y + c = 0.
    pub fn homogeneous(&self) -> [f64; 3] {
        [self.phi.cos(), self.phi.sin(), -self.rho]
    }
}

/// Dominant orientation (degrees, in [-45, 45)) of near-horizontal structure,
/// from the sharpness of projection profiles (sum of squared bin counts).
/// `vertical` estimates the deviation of near-vertical structure instead.
pub fn dominant_skew(ink: &Ink, vertical: bool, lo: f64, hi: f64, step: f64) -> (f64, f64) {
    let pts: Vec<(f64, f64)> = sample_points(ink, 60_000);
    let diag = ((ink.w * ink.w + ink.h * ink.h) as f64).sqrt();
    let nb = (2.0 * diag) as usize + 4;
    let mut best = (0.0, f64::MIN);
    let mut a = lo;
    let mut hist = vec![0u32; nb];
    while a <= hi + 1e-9 {
        let t = a.to_radians();
        // project onto the normal of a line with direction angle t (horizontal family)
        // or t+90° (vertical family)
        let (s, c) = if vertical { ((t + std::f64::consts::FRAC_PI_2).sin(), (t + std::f64::consts::FRAC_PI_2).cos()) } else { (t.sin(), t.cos()) };
        hist.iter_mut().for_each(|v| *v = 0);
        for &(x, y) in &pts {
            let r = -x * s + y * c + diag;
            let i = r as usize;
            if i < nb {
                hist[i] += 1;
            }
        }
        let score: f64 = hist.iter().map(|&v| (v as f64) * (v as f64)).sum();
        if score > best.1 {
            best = (a, score);
        }
        a += step;
    }
    best
}

fn sample_points(ink: &Ink, max: usize) -> Vec<(f64, f64)> {
    let n = ink.count().max(1);
    let stride = n.div_ceil(max).max(1);
    let mut out = Vec::with_capacity(n / stride + 1);
    let mut k = 0usize;
    for (i, b) in ink.bits.iter().enumerate() {
        if *b {
            if k.is_multiple_of(stride) {
                out.push(((i % ink.w) as f64 + 0.5, (i / ink.w) as f64 + 0.5));
            }
            k += 1;
        }
    }
    out
}

/// Hough transform restricted to direction angles in [center-range, center+range]
/// (degrees, direction of the line, 0 = horizontal). Returns verified segments
/// at least `min_len` px long, strongest first.
pub fn hough_segments(ink: &Ink, center_deg: f64, range_deg: f64, step_deg: f64, min_len: f64, max_lines: usize) -> Vec<Segment> {
    let pts = sample_points(ink, 60_000);
    let diag = ((ink.w * ink.w + ink.h * ink.h) as f64).sqrt();
    let nr = (2.0 * diag).ceil() as usize + 2;
    let na = ((2.0 * range_deg / step_deg).round() as usize) + 1;
    let angles: Vec<f64> = (0..na).map(|i| center_deg - range_deg + i as f64 * step_deg).collect();
    // normal angle phi = direction + 90°
    let trig: Vec<(f64, f64)> = angles
        .iter()
        .map(|d| {
            let phi = (d + 90.0).to_radians();
            (phi.cos(), phi.sin())
        })
        .collect();
    let mut acc = vec![0u32; na * nr];
    for &(x, y) in &pts {
        for (ai, &(c, s)) in trig.iter().enumerate() {
            let r = (x * c + y * s + diag).round() as usize;
            if r < nr {
                acc[ai * nr + r] += 1;
            }
        }
    }
    let scale = ink.count() as f64 / pts.len().max(1) as f64;
    let min_votes = (min_len * 0.5 / scale).max(8.0) as u32;
    // Local maxima.
    let mut peaks: Vec<(u32, usize, usize)> = Vec::new();
    let (wa, wr) = ((1.0 / step_deg).ceil() as i64, 4i64);
    for ai in 0..na {
        for ri in 0..nr {
            let v = acc[ai * nr + ri];
            if v < min_votes {
                continue;
            }
            let mut is_max = true;
            'outer: for da in -wa..=wa {
                for dr in -wr..=wr {
                    if da == 0 && dr == 0 {
                        continue;
                    }
                    let (a2, r2) = (ai as i64 + da, ri as i64 + dr);
                    if a2 < 0 || r2 < 0 || a2 >= na as i64 || r2 >= nr as i64 {
                        continue;
                    }
                    let u = acc[a2 as usize * nr + r2 as usize];
                    if u > v || (u == v && (da, dr) < (0, 0)) {
                        is_max = false;
                        break 'outer;
                    }
                }
            }
            if is_max {
                peaks.push((v, ai, ri));
            }
        }
    }
    peaks.sort_by(|a, b| b.0.cmp(&a.0));
    let mut segs: Vec<Segment> = Vec::new();
    for (v, ai, ri) in peaks.into_iter().take(max_lines * 4) {
        let phi = (angles[ai] + 90.0).to_radians();
        let rho = ri as f64 - diag;
        for (a, b) in line_runs(ink, phi, rho, 4.0, min_len) {
            // Skip near-duplicates of stronger lines (same line, overlapping extent).
            let dup = segs.iter().any(|s| {
                (s.phi - phi).abs() < 1.5f64.to_radians()
                    && (s.rho - rho).abs() < 4.0
                    && overlap_1d(s, a, b) > 0.5 * a.dist(b).min(s.len())
            });
            if !dup {
                segs.push(Segment { a, b, phi, rho, votes: v });
            }
        }
        if segs.len() >= max_lines {
            break;
        }
    }
    segs.sort_by(|a, b| b.len().partial_cmp(&a.len()).unwrap());
    segs.truncate(max_lines);
    segs
}

/// A segment re-fitted to the centre of its ink stroke, with straightness measures.
#[derive(Clone, Copy, Debug)]
pub struct Refined {
    pub seg: Segment,
    /// RMS distance (px) of the stroke centre from the fitted line (after trimming).
    pub rms: f64,
    /// Largest mean offset (px) of any quarter of the stroke from the fitted line:
    /// near 0 for straight lines, large for chords of curves (systematic bending).
    pub bend: f64,
    /// Fraction of positions along the segment where a clean stroke centre was measured.
    pub coverage: f64,
}

/// Fit a line to the sub-pixel centre of the ink stroke under `seg`.
///
/// Hough segments are only accurate to the angle step and accept any ink inside a
/// ±1 px band, so chords of gently curved data (sine flanks, decaying tails) pass
/// as "lines". Measuring the stroke centre at every pixel along the segment and
/// fitting it exposes both the exact angle and any systematic bending.
pub fn refine_segment(g: &GrayImage, ink: &Ink, seg: &Segment) -> Option<Refined> {
    const R: i64 = 3;
    let len = seg.len();
    if len < 8.0 {
        return None;
    }
    let d = seg.b.sub(seg.a).scale(1.0 / len);
    let n = Pt::new(-d.y, d.x);
    let raw = g.as_raw();
    let mut samples: Vec<(f64, f64)> = Vec::with_capacity(len as usize + 1);
    let steps = len.floor() as usize;
    for i in 0..=steps {
        let t = i as f64;
        let p = seg.a.add(d.scale(t));
        let (mut sw, mut so, mut cnt) = (0.0, 0.0, 0);
        let (mut lo_edge, mut hi_edge) = (false, false);
        for o in -R..=R {
            let q = p.add(n.scale(o as f64));
            let (x, y) = (q.x.floor() as i64, q.y.floor() as i64);
            if !ink.get(x, y) {
                continue;
            }
            let w = (255.0 - raw[y as usize * ink.w + x as usize] as f64).max(1.0);
            sw += w;
            so += w * o as f64;
            cnt += 1;
            lo_edge |= o == -R;
            hi_edge |= o == R;
        }
        // Skip gaps and places where something crosses the stroke (ticks, text,
        // other curves): the centroid there says nothing about this line.
        if cnt == 0 || cnt > 5 || (lo_edge && hi_edge) {
            continue;
        }
        samples.push((t, so / sw));
    }
    if samples.len() < 8 || (samples.len() as f64) < 0.4 * (steps + 1) as f64 {
        return None;
    }
    let fit = |pts: &[(f64, f64)]| -> Option<(f64, f64)> {
        let n = pts.len() as f64;
        let (st, so) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mt, mo) = (st / n, so / n);
        let (mut stt, mut sto) = (0.0, 0.0);
        for p in pts {
            stt += (p.0 - mt) * (p.0 - mt);
            sto += (p.0 - mt) * (p.1 - mo);
        }
        (stt > 1e-9).then(|| {
            let beta = sto / stt;
            (mo - beta * mt, beta)
        })
    };
    let (mut alpha, mut beta) = fit(&samples)?;
    // Two trimming passes drop crossings that slipped through.
    let mut kept = samples.clone();
    for _ in 0..2 {
        kept = samples.iter().copied().filter(|p| (p.1 - alpha - beta * p.0).abs() <= 1.5).collect();
        if kept.len() < 8 {
            return None;
        }
        (alpha, beta) = fit(&kept)?;
    }
    let res = |p: &(f64, f64)| p.1 - alpha - beta * p.0;
    let rms = (kept.iter().map(|p| res(p).powi(2)).sum::<f64>() / kept.len() as f64).sqrt();
    let mut bend: f64 = 0.0;
    for q in 0..4 {
        let (t0, t1) = (len * q as f64 / 4.0, len * (q + 1) as f64 / 4.0);
        let part: Vec<f64> = kept.iter().filter(|p| p.0 >= t0 && p.0 < t1 + 1e-9).map(res).collect();
        if part.len() >= 4 {
            bend = bend.max((part.iter().sum::<f64>() / part.len() as f64).abs());
        }
    }
    let a = seg.a.add(n.scale(alpha));
    let b = seg.a.add(d.scale(len)).add(n.scale(alpha + beta * len));
    let dir = b.sub(a);
    let phi = dir.y.atan2(dir.x) + std::f64::consts::FRAC_PI_2;
    let rho = a.x * phi.cos() + a.y * phi.sin();
    Some(Refined {
        seg: Segment { a, b, phi, rho, votes: seg.votes },
        rms,
        bend,
        coverage: kept.len() as f64 / (steps + 1) as f64,
    })
}

/// How much darker the segment is than its surroundings on its *less* bright
/// side (median along the segment, grey levels). A drawn line (axis, frame,
/// grid) is darker than the background on both sides; a step edge (the border
/// of a photographed page, the side of a filled bar) has one side as dark as
/// the "line" itself and scores near 0. Sides falling outside the image are
/// ignored; returns `None` if nothing could be measured.
pub fn stroke_contrast(g: &GrayImage, seg: &Segment) -> Option<f64> {
    let len = seg.len();
    if len < 1e-9 {
        return None;
    }
    let d = seg.b.sub(seg.a).scale(1.0 / len);
    let n = Pt::new(-d.y, d.x);
    let px = |p: Pt| -> Option<f64> {
        let (x, y) = (p.x.floor() as i64, p.y.floor() as i64);
        (x >= 0 && y >= 0 && x < g.width() as i64 && y < g.height() as i64).then(|| g.get_pixel(x as u32, y as u32).0[0] as f64)
    };
    let mut vals = Vec::with_capacity(48);
    for i in 0..48 {
        let p = seg.a.add(d.scale(len * (i as f64 + 0.5) / 48.0));
        // Darkest pixel on the line (Hough lines are only ±1 px accurate) and
        // brightest pixel just beyond the stroke on each side (thick strokes).
        let Some(line) = (-1..=1).filter_map(|o| px(p.add(n.scale(o as f64)))).reduce(f64::min) else { continue };
        let side = |s: f64| (3..=6).filter_map(|o| px(p.add(n.scale(s * o as f64)))).reduce(f64::max);
        let Some(lo) = [side(-1.0), side(1.0)].into_iter().flatten().reduce(f64::min) else { continue };
        vals.push(lo - line);
    }
    if vals.is_empty() {
        return None;
    }
    vals.sort_by(|a, b| a.total_cmp(b));
    Some(vals[vals.len() / 2])
}

/// Length of the overlap of segment `s` with the segment (a, b) projected on s's direction.
fn overlap_1d(s: &Segment, a: Pt, b: Pt) -> f64 {
    let d = s.b.sub(s.a);
    let l = d.norm().max(1e-9);
    let u = d.scale(1.0 / l);
    let (p0, p1) = (0.0f64, l);
    let (q0, q1) = {
        let ta = a.sub(s.a).dot(u);
        let tb = b.sub(s.a).dot(u);
        (ta.min(tb), ta.max(tb))
    };
    (p1.min(q1) - p0.max(q0)).max(0.0)
}

/// All runs of ink along the line at least `min_len` long (gaps up to `max_gap`).
pub fn line_runs(ink: &Ink, phi: f64, rho: f64, max_gap: f64, min_len: f64) -> Vec<(Pt, Pt)> {
    let (c, s) = (phi.cos(), phi.sin());
    let p0 = Pt::new(c * rho, s * rho);
    let d = Pt::new(-s, c);
    let diag = ((ink.w * ink.w + ink.h * ink.h) as f64).sqrt();
    let mut runs = Vec::new();
    let mut run_start: Option<f64> = None;
    let mut last_hit = f64::NEG_INFINITY;
    let mut t = -diag;
    let (cx, cy) = (c.round() as i64, s.round() as i64);
    while t <= diag + 1.0 {
        let p = p0.add(d.scale(t));
        let (xi, yi) = (p.x.floor() as i64, p.y.floor() as i64);
        let hit = t <= diag && (ink.get(xi, yi) || ink.get(xi + cx, yi + cy) || ink.get(xi - cx, yi - cy));
        if hit {
            if run_start.is_some() && t - last_hit > max_gap {
                let rs = run_start.unwrap();
                if last_hit - rs >= min_len {
                    runs.push((p0.add(d.scale(rs)), p0.add(d.scale(last_hit))));
                }
                run_start = Some(t);
            } else if run_start.is_none() {
                run_start = Some(t);
            }
            last_hit = t;
        }
        t += 1.0;
    }
    if let Some(rs) = run_start {
        if last_hit - rs >= min_len {
            runs.push((p0.add(d.scale(rs)), p0.add(d.scale(last_hit))));
        }
    }
    runs
}

/// Walk along the line, find the longest run of ink (gaps up to `max_gap` px).
pub fn verify_line(ink: &Ink, phi: f64, rho: f64, max_gap: f64) -> Option<(Pt, Pt)> {
    let (c, s) = (phi.cos(), phi.sin());
    // A point on the line and its direction.
    let p0 = Pt::new(c * rho, s * rho);
    let d = Pt::new(-s, c);
    let diag = ((ink.w * ink.w + ink.h * ink.h) as f64).sqrt();
    let mut best: Option<(f64, f64)> = None;
    let mut run_start: Option<f64> = None;
    let mut last_hit = f64::NEG_INFINITY;
    let mut t = -diag;
    while t <= diag {
        let p = p0.add(d.scale(t));
        let (xi, yi) = (p.x.floor() as i64, p.y.floor() as i64);
        let hit = ink.get(xi, yi)
            || ink.get(xi + (c.round() as i64), yi + (s.round() as i64))
            || ink.get(xi - (c.round() as i64), yi - (s.round() as i64));
        if hit {
            if run_start.is_none() || t - last_hit > max_gap {
                if let (Some(rs), true) = (run_start, last_hit > f64::NEG_INFINITY) {
                    if best.map(|b| last_hit - rs > b.1 - b.0).unwrap_or(true) {
                        best = Some((rs, last_hit));
                    }
                }
                run_start = Some(t);
            }
            last_hit = t;
        }
        t += 1.0;
    }
    if let Some(rs) = run_start {
        if best.map(|b| last_hit - rs > b.1 - b.0).unwrap_or(true) {
            best = Some((rs, last_hit));
        }
    }
    best.map(|(a, b)| (p0.add(d.scale(a)), p0.add(d.scale(b))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn finds_rotated_rectangle_lines() {
        let (w, h) = (400u32, 300u32);
        let mut g = GrayImage::from_pixel(w, h, Luma([255]));
        let ang = 3f64.to_radians();
        let c = Pt::new(200.0, 150.0);
        let rot = |p: Pt| {
            let q = p.sub(c);
            Pt::new(c.x + q.x * ang.cos() - q.y * ang.sin(), c.y + q.x * ang.sin() + q.y * ang.cos())
        };
        let corners = [Pt::new(60.0, 60.0), Pt::new(340.0, 60.0), Pt::new(340.0, 240.0), Pt::new(60.0, 240.0)].map(rot);
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            for k in 0..=1000 {
                let p = a.lerp(b, k as f64 / 1000.0);
                for dy in 0..2 {
                    let (x, y) = (p.x as u32, p.y as u32 + dy);
                    if x < w && y < h {
                        g.put_pixel(x, y, Luma([0]));
                    }
                }
            }
        }
        let ink = detect_ink(&g, 12, 25.0);
        let (sk, _) = dominant_skew(&ink, false, -45.0, 45.0, 0.5);
        assert!((sk - 3.0).abs() <= 0.5, "skew {sk}");
        let hs = hough_segments(&ink, sk, 6.0, 0.1, 100.0, 10);
        assert!(hs.len() >= 2, "{}", hs.len());
        for s in &hs[..2] {
            assert!((s.angle_deg() - 3.0).abs() < 0.3, "{}", s.angle_deg());
            assert!(s.len() > 250.0);
        }
    }
}
