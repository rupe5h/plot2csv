//! Automatic data-series detection inside a plot panel.

use image::RgbaImage;
use plot2csv_core::axes::{AxesKind, Calibration};
use plot2csv_core::dataset::{Algorithm, AutoSettings, ColorMode, DataPoint, Dataset, Mask, SeriesStyle};
use plot2csv_core::geom::RectF;
use plot2csv_core::project::{Panel, TextItem, TextRole};

use crate::color::{chroma, dominant_colors, lab_dist, to_lab};
use crate::extract::{averaging_window, bars, blobs, Bitmap};
use crate::raster::{component_stats, label_components};

pub struct SeriesResult {
    pub datasets: Vec<Dataset>,
    pub note: String,
}

/// A colour cluster of non-background pixels.
struct Cluster {
    color: [u8; 3],
    lab: [f64; 3],
    bits: Vec<bool>,
    count: usize,
}

fn dist_to_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if l2 > 1e-9 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    lab_dist(p, [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]])
}

/// Difference of Lab hue angles in degrees.
fn hue_diff(a: [f64; 3], b: [f64; 3]) -> f64 {
    let ha = a[2].atan2(a[1]).to_degrees();
    let hb = b[2].atan2(b[1]).to_degrees();
    let d = (ha - hb).abs() % 360.0;
    d.min(360.0 - d)
}

/// One curve found by the column tracker.
#[derive(Clone, Debug)]
pub struct Track {
    pub pts: Vec<(f64, f64)>,
    pub slope: f64,
}

impl Track {
    fn last(&self) -> (f64, f64) {
        *self.pts.last().unwrap()
    }
    fn first(&self) -> (f64, f64) {
        self.pts[0]
    }
    fn span(&self) -> f64 {
        self.last().0 - self.first().0
    }
    /// Fraction of columns between the ends that have a sample (solid ≈ 1).
    pub fn coverage(&self) -> f64 {
        let s = self.span().max(1.0);
        (self.pts.len() as f64 / (s + 1.0)).min(1.0)
    }
}

pub fn color_name(c: [u8; 3]) -> &'static str {
    let lab = to_lab(c);
    let ch = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    if ch < 12.0 {
        return if lab[0] < 30.0 {
            "Black"
        } else if lab[0] < 75.0 {
            "Grey"
        } else {
            "Light grey"
        };
    }
    let (r, g, b) = (c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    match h {
        h if !(15.0..345.0).contains(&h) => "Red",
        h if h < 45.0 => "Orange",
        h if h < 70.0 => "Yellow",
        h if h < 160.0 => "Green",
        h if h < 200.0 => "Cyan",
        h if h < 255.0 => "Blue",
        h if h < 290.0 => "Purple",
        _ => "Magenta",
    }
}

/// Least-squares slope over the last points of a track (x within `window` px).
fn recent_slope(pts: &[(f64, f64)], window: f64) -> f64 {
    let Some(&(lx, _)) = pts.last() else { return 0.0 };
    let recent: Vec<&(f64, f64)> = pts.iter().rev().take_while(|p| lx - p.0 <= window).collect();
    if recent.len() < 3 {
        return 0.0;
    }
    let n = recent.len() as f64;
    let mx = recent.iter().map(|p| p.0).sum::<f64>() / n;
    let my = recent.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = recent.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = recent.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    if sxx < 1e-9 {
        0.0
    } else {
        sxy / sxx
    }
}

/// Column-wise multi-curve tracker over a binary map (function-like curves).
///
/// Each open track predicts its next y from a least-squares slope over its
/// recent points. Tracks whose predictions collide (a crossing) coast without
/// taking points, so they keep their direction through the crossing. Fragments
/// (dashes, dots, interrupted curves) are then linked by position, slope and
/// dash-style similarity.
pub fn track_curves(bm: &Bitmap, region: &RectF, max_jump: f64, max_gap: f64) -> Vec<Track> {
    let x0 = region.x0.max(0.0) as usize;
    let x1 = (region.x1 as usize).min(bm.width);
    let y0 = region.y0.max(0.0) as usize;
    let y1 = (region.y1 as usize).min(bm.height);
    let slope_win = (2.0 * max_gap).max(12.0);
    struct Open {
        t: Track,
        last_x: f64,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut closed: Vec<Track> = Vec::new();
    for x in x0..x1 {
        let mut runs: Vec<(f64, usize)> = Vec::new();
        let mut y = y0;
        while y < y1 {
            if bm.get(x, y) {
                let s = y;
                while y < y1 && bm.get(x, y) {
                    y += 1;
                }
                let len = y - s;
                if (len as f64) < 0.25 * (y1 - y0) as f64 {
                    runs.push(((s + y) as f64 / 2.0, len));
                }
            } else {
                y += 1;
            }
        }
        let xf = x as f64 + 0.5;
        let preds: Vec<f64> = open
            .iter()
            .map(|o| {
                let (px, py) = o.t.last();
                py + o.t.slope * (xf - px)
            })
            .collect();
        // Tracks in a crossing: another track predicts nearly the same y.
        let crossing: Vec<bool> = (0..open.len())
            .map(|i| (0..open.len()).any(|j| j != i && (preds[i] - preds[j]).abs() < 0.8 * max_jump))
            .collect();
        let mut cands: Vec<(f64, usize, usize)> = Vec::new();
        for (ti, o) in open.iter().enumerate() {
            let tol = 0.4 * max_jump + 0.25 * (xf - o.last_x).abs().min(3.0 * max_gap);
            for (ri, &(ry, _)) in runs.iter().enumerate() {
                let d = (ry - preds[ti]).abs();
                if d <= tol {
                    cands.push((d, ti, ri));
                }
            }
        }
        cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut t_used = vec![false; open.len()];
        let mut r_used = vec![false; runs.len()];
        for &(_, ti, ri) in &cands {
            if t_used[ti] || r_used[ri] {
                continue;
            }
            // A crossing track only takes a run that is clearly its own: no
            // other colliding track wants the same run.
            if crossing[ti] {
                let contested = cands.iter().any(|&(_, tj, rj)| rj == ri && tj != ti && crossing[tj]);
                if contested {
                    continue;
                }
            }
            t_used[ti] = true;
            r_used[ri] = true;
            let o = &mut open[ti];
            o.t.pts.push((xf, runs[ri].0));
            o.last_x = xf;
            if !crossing[ti] {
                o.t.slope = recent_slope(&o.t.pts, slope_win);
            }
        }
        for (ri, &(ry, _)) in runs.iter().enumerate() {
            // Don't spawn duplicates next to a coasting track.
            if !r_used[ri] && !preds.iter().any(|p| (p - ry).abs() < max_jump) {
                open.push(Open { t: Track { pts: vec![(xf, ry)], slope: 0.0 }, last_x: xf });
            }
        }
        let mut k = 0;
        while k < open.len() {
            if xf - open[k].last_x > max_gap {
                closed.push(open.remove(k).t);
            } else {
                k += 1;
            }
        }
    }
    closed.extend(open.into_iter().map(|o| o.t));
    closed.retain(|t| t.pts.len() >= 2);
    for t in closed.iter_mut() {
        t.slope = recent_slope(&t.pts, slope_win);
    }
    link_fragments(closed, max_jump, max_gap, slope_win)
}

/// Start slope of a fragment (least squares over its first points).
fn start_slope(t: &Track, window: f64) -> f64 {
    let fx = t.first().0;
    let pts: Vec<(f64, f64)> = t.pts.iter().take_while(|p| p.0 - fx <= window).copied().collect();
    recent_slope(&pts, window)
}

fn link_fragments(mut frags: Vec<Track>, max_jump: f64, max_gap: f64, slope_win: f64) -> Vec<Track> {
    frags.sort_by(|a, b| a.first().0.partial_cmp(&b.first().0).unwrap());
    // Greedy: repeatedly take the globally cheapest end->start link.
    loop {
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..frags.len() {
            let (lx, ly) = frags[i].last();
            let si = frags[i].slope;
            let ci = frags[i].coverage();
            for j in 0..frags.len() {
                if i == j {
                    continue;
                }
                let (fx, fy) = frags[j].first();
                let dx = fx - lx;
                if dx <= 0.0 || dx > 8.0 * max_gap {
                    continue;
                }
                let pred = ly + si * dx;
                let dy = (fy - pred).abs();
                if dy > max_jump + 0.12 * dx {
                    continue;
                }
                let sj = if frags[j].pts.len() >= 3 { start_slope(&frags[j], slope_win) } else { si };
                let dslope = ((sj.atan() - si.atan()).abs()).to_degrees();
                if dslope > 35.0 && frags[j].pts.len() >= 4 && frags[i].pts.len() >= 4 {
                    continue;
                }
                // Dash style: short fragments with gaps should link to similar ones.
                let cj = frags[j].coverage();
                let style = (ci - cj).abs();
                let cost = dy / max_jump + dx / (4.0 * max_gap) + dslope / 30.0 + 2.0 * style;
                if best.map(|b| cost < b.0).unwrap_or(true) {
                    best = Some((cost, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        let nxt = frags.remove(j);
        let i = if j < i { i - 1 } else { i };
        let cur = &mut frags[i];
        cur.pts.extend(nxt.pts);
        cur.slope = recent_slope(&cur.pts, slope_win);
    }
    frags
}

/// A light-grey, perfectly straight horizontal track is a grid line, not data.
fn is_grid_like(t: &Track, color: [u8; 3]) -> bool {
    let lab = to_lab(color);
    if chroma(color) > 12.0 || lab[0] < 45.0 || t.pts.len() < 5 {
        return false;
    }
    let n = t.pts.len() as f64;
    let mx = t.pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = t.pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = t.pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = t.pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let b = if sxx > 1e-9 { sxy / sxx } else { 0.0 };
    let rms = (t.pts.iter().map(|p| (p.1 - (my + b * (p.0 - mx))).powi(2)).sum::<f64>() / n).sqrt();
    b.abs() < 0.03 && rms < 1.2
}

/// Join tracks that do not overlap in x (pieces of one curve of this colour).
fn join_disjoint(mut tracks: Vec<Track>) -> Vec<Track> {
    tracks.sort_by(|a, b| b.pts.len().cmp(&a.pts.len()));
    let mut out: Vec<Track> = Vec::new();
    for t in tracks {
        let (a0, a1) = (t.first().0, t.last().0);
        let target = out.iter_mut().find(|o| {
            let (b0, b1) = (o.first().0, o.last().0);
            let overlap = (a1.min(b1) - a0.max(b0)).max(0.0);
            overlap < 0.1 * (a1 - a0).min(b1 - b0).max(1.0)
        });
        match target {
            Some(o) => {
                o.pts.extend(t.pts);
                o.pts.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
            }
            None => out.push(t),
        }
    }
    out
}

/// Downsample a track to points roughly `dx` apart (averaging within each bin).
fn track_points(t: &Track, dx: f64) -> Vec<DataPoint> {
    let mut out = Vec::new();
    let mut bin_start = t.first().0;
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for &(x, y) in &t.pts {
        if x - bin_start >= dx && n > 0.0 {
            out.push(DataPoint::new(sx / n, sy / n));
            bin_start = x;
            sx = 0.0;
            sy = 0.0;
            n = 0.0;
        }
        sx += x;
        sy += y;
        n += 1.0;
    }
    if n > 0.0 {
        out.push(DataPoint::new(sx / n, sy / n));
    }
    out
}

/// Mask: a band of half-width `r` around a track.
fn band_mask(t: &Track, w: usize, h: usize, r: f64) -> Mask {
    let mut bits = vec![false; w * h];
    let ri = r.ceil() as i64;
    let pts = &t.pts;
    let mut paint = |x: f64, y: f64| {
        for dy in -ri..=ri {
            let yy = y as i64 + dy;
            if yy < 0 || yy >= h as i64 {
                continue;
            }
            for dx in -1..=1i64 {
                let xx = x as i64 + dx;
                if xx >= 0 && (xx as usize) < w {
                    bits[yy as usize * w + xx as usize] = true;
                }
            }
        }
    };
    for k in 0..pts.len() {
        paint(pts[k].0, pts[k].1);
        if k + 1 < pts.len() {
            let (a, b) = (pts[k], pts[k + 1]);
            let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize;
            for s in 1..steps {
                let f = s as f64 / steps as f64;
                paint(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            }
        }
    }
    Mask::from_bits(w as u32, h as u32, &bits)
}

fn rect_mask(r: &RectF, w: usize, h: usize) -> Mask {
    let mut bits = vec![false; w * h];
    for y in (r.y0.max(0.0) as usize)..(r.y1.min(h as f64) as usize) {
        for x in (r.x0.max(0.0) as usize)..(r.x1.min(w as f64) as usize) {
            bits[y * w + x] = true;
        }
    }
    Mask::from_bits(w as u32, h as u32, &bits)
}

pub fn detect_series(img: &RgbaImage, panel: &Panel, items: &[TextItem], cal: Option<&Calibration>) -> SeriesResult {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let inner = panel.inner;
    let region = RectF::new(inner.x0 + 3.0, inner.y0 + 3.0, inner.x1 - 3.0, inner.y1 - 3.0).clamp_to(w as f64, h as f64);
    if region.width() < 10.0 || region.height() < 10.0 {
        return SeriesResult { datasets: vec![], note: "The plot area is too small.".into() };
    }
    let (rx0, ry0, rx1, ry1) = (region.x0 as usize, region.y0 as usize, region.x1 as usize, region.y1 as usize);
    let raw = img.as_raw();
    let px = |x: usize, y: usize| -> [u8; 3] {
        let i = 4 * (y * w + x);
        [raw[i], raw[i + 1], raw[i + 2]]
    };
    // Background = most common colour in the region.
    let dom = dominant_colors(img, Some(region), 12, 8.0);
    let bg = dom.first().map(|c| c.color).unwrap_or([255, 255, 255]);
    let bg_lab = to_lab(bg);

    // Exclusions: text boxes (legend, labels inside the plot) and legend swatches.
    let mut excl: Vec<RectF> = Vec::new();
    let mut legend: Vec<(RectF, String)> = Vec::new();
    for it in items {
        let c = it.bbox.center();
        if region.contains(c) || it.role == TextRole::Legend {
            excl.push(it.bbox.expand(2.0));
        }
        if it.role == TextRole::Legend {
            let th = it.bbox.height().max(6.0);
            let sw = RectF::new(it.bbox.x0 - 4.0 * th, it.bbox.y0, it.bbox.x0 - 1.0, it.bbox.y1);
            excl.push(sw.expand(2.0));
            legend.push((sw, it.text.clone()));
        }
    }
    // Merge legend entries into one box (with frame) when they form a column.
    if legend.len() >= 2 {
        let x0 = legend.iter().map(|l| l.0.x0).fold(f64::MAX, f64::min);
        let y0 = legend.iter().map(|l| l.0.y0).fold(f64::MAX, f64::min);
        let x1 = items.iter().filter(|i| i.role == TextRole::Legend).map(|i| i.bbox.x1).fold(f64::MIN, f64::max);
        let y1 = legend.iter().map(|l| l.0.y1).fold(f64::MIN, f64::max);
        excl.push(RectF::new(x0, y0, x1, y1).expand(8.0));
    }
    let excluded = |x: usize, y: usize| {
        let p = plot2csv_core::geom::Pt::new(x as f64 + 0.5, y as f64 + 0.5);
        excl.iter().any(|r| r.contains(p))
    };

    // Candidate ink: far enough from the background colour.
    let mut cand = vec![false; w * h];
    let mut n_cand = 0usize;
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if excluded(x, y) {
                continue;
            }
            if lab_dist(to_lab(px(x, y)), bg_lab) > 22.0 {
                cand[y * w + x] = true;
                n_cand += 1;
            }
        }
    }
    // Grid / frame lines: rows or columns mostly covered by low-chroma ink.
    let rw = (rx1 - rx0) as f64;
    let rh = (ry1 - ry0) as f64;
    let mut grid_rows = Vec::new();
    for y in ry0..ry1 {
        let n = (rx0..rx1).filter(|&x| cand[y * w + x] && chroma(px(x, y)) < 14.0).count();
        if n as f64 > 0.6 * rw {
            grid_rows.push(y);
        }
    }
    let mut grid_cols = Vec::new();
    for x in rx0..rx1 {
        let n = (ry0..ry1).filter(|&y| cand[y * w + x] && chroma(px(x, y)) < 14.0).count();
        if n as f64 > 0.6 * rh {
            grid_cols.push(x);
        }
    }
    for &y in &grid_rows {
        for x in rx0..rx1 {
            // Keep pixels that continue vertically (a curve crossing the grid line).
            let i = y * w + x;
            if cand[i] && chroma(px(x, y)) < 14.0 {
                let up = y > 1 && cand[(y - 2) * w + x];
                let dn = y + 2 < h && cand[(y + 2) * w + x];
                if !(up && dn) {
                    cand[i] = false;
                }
            }
        }
    }
    for &x in &grid_cols {
        for y in ry0..ry1 {
            let i = y * w + x;
            if cand[i] && chroma(px(x, y)) < 14.0 {
                let l = x > 1 && cand[y * w + x - 2];
                let r = x + 2 < w && cand[y * w + x + 2];
                if !(l && r) {
                    cand[i] = false;
                }
            }
        }
    }

    // Colour clusters among candidate pixels.
    let mut tmp = RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([bg[0], bg[1], bg[2], 255]));
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if cand[y * w + x] {
                let c = px(x, y);
                tmp.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }
    let raw_colors: Vec<([u8; 3], usize)> = dominant_colors(&tmp, Some(region), 16, 14.0)
        .into_iter()
        .filter(|c| lab_dist(to_lab(c.color), bg_lab) > 22.0)
        .filter(|c| c.count >= 30 && c.count as f64 >= 0.004 * n_cand.max(1) as f64)
        .map(|c| (c.color, c.count))
        .collect();
    // Merge shades of one series: similar colours, or a colour lying between a
    // stronger cluster and the background (anti-aliased edges, blur, JPEG).
    let mut accepted: Vec<([u8; 3], usize)> = Vec::new();
    for (c, n) in raw_colors {
        let l = to_lab(c);
        let merge = accepted.iter_mut().find(|(a, an)| {
            let al = to_lab(*a);
            let d = lab_dist(l, al);
            let seg = dist_to_segment(l, al, bg_lab);
            let hue_close = (chroma(c) > 15.0 && chroma(*a) > 15.0 && hue_diff(l, al) < 18.0 && d < 45.0) || d < 22.0;
            hue_close || (seg < 10.0 && (n as f64) < 0.6 * *an as f64)
        });
        match merge {
            Some(a) => a.1 += n,
            None => accepted.push((c, n)),
        }
    }
    let colors: Vec<[u8; 3]> = accepted.into_iter().map(|a| a.0).collect();
    let mut clusters: Vec<Cluster> =
        colors.iter().map(|&c| Cluster { color: c, lab: to_lab(c), bits: vec![false; w * h], count: 0 }).collect();
    if clusters.is_empty() {
        return SeriesResult { datasets: vec![], note: "No data-coloured pixels found inside the plot area.".into() };
    }
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * w + x;
            if !cand[i] {
                continue;
            }
            let l = to_lab(px(x, y));
            let mut best = (f64::MAX, 0usize);
            for (k, c) in clusters.iter().enumerate() {
                let d = lab_dist(l, c.lab);
                if d < best.0 {
                    best = (d, k);
                }
            }
            // Anti-aliased edges are closer to the background: still assign if
            // clearly nearer to the cluster than to the background.
            if best.0 < 35.0 || best.0 < 0.6 * lab_dist(l, bg_lab) {
                clusters[best.1].bits[i] = true;
                clusters[best.1].count += 1;
            }
        }
    }
    clusters.retain(|c| c.count >= 25);

    // Legend colours -> names.
    let legend_colors: Vec<([f64; 3], String)> = legend
        .iter()
        .filter_map(|(sw, name)| {
            let mut best: Option<([u8; 3], f64)> = None;
            for y in sw.y0.max(0.0) as usize..(sw.y1 as usize).min(h) {
                for x in sw.x0.max(0.0) as usize..(sw.x1 as usize).min(w) {
                    let c = px(x, y);
                    let d = lab_dist(to_lab(c), bg_lab);
                    if d > 22.0 && best.map(|b| d > b.1).unwrap_or(true) {
                        best = Some((c, d));
                    }
                }
            }
            best.map(|(c, _)| (to_lab(c), name.clone()))
        })
        .collect();
    let name_for = |lab: [f64; 3]| -> Option<String> {
        legend_colors
            .iter()
            .map(|(l, n)| (lab_dist(*l, lab), n))
            .filter(|(d, _)| *d < 30.0)
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .map(|(_, n)| n.clone())
    };

    let dx = (0.01 * inner.width()).clamp(4.0, 20.0);
    let max_jump = (0.03 * inner.height()).max(6.0);
    let max_gap = (0.03 * inner.width()).max(8.0);
    let mut datasets: Vec<Dataset> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for c in &clusters {
        let bm = Bitmap { bits: &c.bits, width: w, height: h };
        let base_name = name_for(c.lab).unwrap_or_else(|| color_name(c.color).to_string());
        let mut auto = AutoSettings {
            color_mode: ColorMode::Foreground,
            fg: c.color,
            bg,
            distance: 60.0,
            algorithm: Algorithm::AveragingWindow { dx, dy: dx },
            mask: None,
        };
        // Shape analysis of the cluster's components.
        let (labels, n) = label_components(&c.bits, w, h, true);
        let stats = component_stats(&labels, n, w);
        let comps: Vec<_> = stats.iter().skip(1).filter(|s| s.area > 0).collect();
        let small_compact: Vec<_> = comps
            .iter()
            .filter(|s| {
                let (bw, bh) = (s.width() as f64, s.height() as f64);
                let fill = s.area as f64 / (bw * bh);
                bw < 0.06 * inner.width() && bh < 0.06 * inner.height() && bw / bh > 0.4 && bw / bh < 2.5 && fill > 0.3 && s.area >= 6
            })
            .collect();
        let compact_area: usize = small_compact.iter().map(|s| s.area).sum();
        let is_markers = small_compact.len() >= 4 && compact_area as f64 > 0.7 * c.count as f64;

        if panel.kind == AxesKind::Bar {
            let pts = bars(&bm, cal, 3.0).unwrap_or_default();
            if pts.len() >= 1 {
                let mut ds = Dataset::new(base_name.clone());
                ds.color = c.color;
                ds.style = SeriesStyle::Bars;
                auto.algorithm = Algorithm::Bar { min_width: 3.0 };
                auto.mask = Some(rect_mask(&region, w, h));
                ds.points = label_bars(pts, items, &inner);
                ds.auto = auto;
                notes.push(format!("{}: {} bars", ds.name, ds.points.len()));
                datasets.push(ds);
            }
            continue;
        }
        if is_markers {
            let sizes: Vec<f64> = small_compact.iter().map(|s| 2.0 * (s.area as f64 / std::f64::consts::PI).sqrt()).collect();
            let mut sorted = sizes.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = sorted[sorted.len() / 2];
            let (min_d, max_d) = (0.5 * med, 1.8 * med);
            let mut ds = Dataset::new(base_name.clone());
            ds.color = c.color;
            ds.style = SeriesStyle::Markers;
            ds.points = blobs(&bm, min_d, max_d);
            auto.algorithm = Algorithm::Blob { min_d, max_d };
            auto.mask = Some(rect_mask(&region, w, h));
            ds.auto = auto;
            notes.push(format!("{}: {} markers", ds.name, ds.points.len()));
            datasets.push(ds);
            continue;
        }
        // Lines: track curves; several tracks of one colour become separate series.
        let tracks: Vec<Track> = track_curves(&bm, &region, max_jump, max_gap)
            .into_iter()
            .filter(|t| t.span() >= 0.05 * region.width() && t.pts.len() >= 6)
            .collect();
        let tracks: Vec<Track> = join_disjoint(tracks)
            .into_iter()
            .filter(|t| t.span() >= 0.12 * region.width() && t.pts.len() >= 10 && !is_grid_like(t, c.color))
            .collect();
        if tracks.is_empty() && chroma(c.color) < 12.0 && to_lab(c.color)[0] >= 45.0 {
            continue;
        }
        if tracks.len() <= 1 {
            let mut ds = Dataset::new(base_name.clone());
            ds.color = c.color;
            ds.style = match tracks.first() {
                Some(t) if t.coverage() < 0.85 => SeriesStyle::Dashed,
                _ => SeriesStyle::Line,
            };
            ds.points = match tracks.first() {
                Some(t) => track_points(t, dx),
                None => averaging_window(&bm, dx, dx),
            };
            auto.mask = Some(match tracks.first() {
                Some(t) => band_mask(t, w, h, max_jump.max(5.0)),
                None => rect_mask(&region, w, h),
            });
            ds.auto = auto;
            let span = ds.points.iter().map(|p| p.x).fold(f64::MIN, f64::max) - ds.points.iter().map(|p| p.x).fold(f64::MAX, f64::min);
            if ds.points.len() >= 6 && span >= 0.1 * region.width() {
                notes.push(format!("{}: {} points", ds.name, ds.points.len()));
                datasets.push(ds);
            }
        } else {
            for (k, t) in tracks.iter().enumerate() {
                let cov = t.coverage();
                let style = if cov > 0.85 { SeriesStyle::Line } else { SeriesStyle::Dashed };
                let word = if cov > 0.85 {
                    "solid"
                } else if cov > 0.5 {
                    "dashed"
                } else {
                    "dotted"
                };
                let mut ds = Dataset::new(format!("{} {} ({word})", base_name, k + 1));
                ds.color = c.color;
                ds.style = style;
                ds.points = track_points(t, dx);
                let mut a = auto.clone();
                a.mask = Some(band_mask(t, w, h, (0.5 * max_jump).max(4.0)));
                ds.auto = a;
                notes.push(format!("{}: {} points", ds.name, ds.points.len()));
                datasets.push(ds);
            }
        }
    }
    // Order: legend order first, then by colour count.
    let note = if datasets.is_empty() {
        "No series found automatically; add points by hand or adjust the colour settings.".to_string()
    } else {
        format!("Found {} series — {}.", datasets.len(), notes.join("; "))
    };
    SeriesResult { datasets, note }
}

/// Attach category labels (text under each bar) to bar points.
fn label_bars(mut pts: Vec<DataPoint>, items: &[TextItem], inner: &RectF) -> Vec<DataPoint> {
    for p in pts.iter_mut() {
        let best = items
            .iter()
            .filter(|it| it.bbox.center().y > inner.y1 && it.value.is_none())
            .map(|it| ((it.bbox.center().x - p.x).abs(), it))
            .filter(|(d, it)| *d < it.bbox.width().max(20.0))
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if let Some((_, it)) = best {
            p.label = Some(it.text.clone());
        }
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use plot2csv_core::geom::RectF;

    fn panel(inner: RectF) -> Panel {
        Panel { id: 1, name: "p".into(), kind: AxesKind::Xy, inner, outer: inner.expand(30.0), enabled: true }
    }

    fn draw_curve(img: &mut RgbaImage, f: impl Fn(f64) -> f64, x0: u32, x1: u32, col: [u8; 3], dash: Option<(u32, u32)>, thick: u32) {
        for x in x0..x1 {
            if let Some((on, off)) = dash {
                if (x - x0) % (on + off) >= on {
                    continue;
                }
            }
            let y = f(x as f64);
            for t in 0..thick {
                img.put_pixel(x, (y as u32 + t).min(img.height() - 1), Rgba([col[0], col[1], col[2], 255]));
            }
        }
    }

    #[test]
    fn two_colored_lines_and_markers() {
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        draw_curve(&mut img, |x| 200.0 + 80.0 * (x / 60.0).sin(), 60, 560, [255, 140, 0], None, 3);
        draw_curve(&mut img, |x| 120.0 + 0.2 * x, 60, 560, [30, 60, 220], None, 2);
        for k in 0..8 {
            let (cx, cy) = (90 + k * 60, 330 - k * 5);
            for y in cy - 3..=cy + 3 {
                for x in cx - 3..=cx + 3 {
                    img.put_pixel(x, y, Rgba([0, 160, 60, 255]));
                }
            }
        }
        let r = detect_series(&img, &panel(inner), &[], None);
        assert_eq!(r.datasets.len(), 3, "{}", r.note);
        let markers = r.datasets.iter().find(|d| d.style == SeriesStyle::Markers).expect(&r.note);
        assert_eq!(markers.points.len(), 8);
        let orange = r.datasets.iter().find(|d| d.name == "Orange").expect(&r.note);
        for p in &orange.points {
            let expect = 200.0 + 80.0 * (p.x / 60.0).sin() + 1.0;
            assert!((p.y - expect).abs() < 3.0, "{p:?} {expect}");
        }
    }

    #[test]
    fn same_color_solid_and_dashed_are_separated() {
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        // Two black curves that cross; one dashed.
        draw_curve(&mut img, |x| 100.0 + 0.4 * x, 60, 560, [0, 0, 0], None, 2);
        draw_curve(&mut img, |x| 330.0 - 0.4 * x, 60, 560, [0, 0, 0], Some((12, 8)), 2);
        let r = detect_series(&img, &panel(inner), &[], None);
        assert_eq!(r.datasets.len(), 2, "{}", r.note);
        let dashed = r.datasets.iter().find(|d| d.style == SeriesStyle::Dashed).expect(&r.note);
        let solid = r.datasets.iter().find(|d| d.style == SeriesStyle::Line).expect(&r.note);
        for p in &dashed.points {
            assert!((p.y - (330.0 - 0.4 * p.x + 0.5)).abs() < 3.0, "dashed {p:?}");
        }
        for p in &solid.points {
            assert!((p.y - (100.0 + 0.4 * p.x + 0.5)).abs() < 3.0, "solid {p:?}");
        }
        assert!(dashed.points.len() > 20 && solid.points.len() > 20);
    }
}
