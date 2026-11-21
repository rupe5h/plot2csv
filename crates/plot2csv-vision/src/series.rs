//! Automatic data-series detection inside a plot panel.

use image::RgbaImage;
use plot2csv_core::axes::{AxesKind, Calibration};
use plot2csv_core::dataset::{Algorithm, AutoSettings, ColorMode, DataPoint, Dataset, Mask, SeriesStyle};
use plot2csv_core::geom::RectF;
use plot2csv_core::project::{Panel, TextItem, TextRole};

use crate::color::{chroma, dist_to_segment, dominant_colors, fade_path, lab_dist, path_dist, to_lab};
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
    /// Lab polyline of the colour blended into the background (its
    /// anti-aliased edge shades), from the colour itself to the background.
    fade: Vec<[f64; 3]>,
    bits: Vec<bool>,
    count: usize,
}

/// How far a pixel is from being `c` or one of its anti-aliased edge shades.
/// The small direct-distance term splits ties between tints of one hue.
fn fade_dist(p: [f64; 3], c: &Cluster) -> f64 {
    path_dist(p, &c.fade) + 0.1 * lab_dist(p, c.lab)
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
        let crossing: Vec<bool> =
            (0..open.len()).map(|i| (0..open.len()).any(|j| j != i && (preds[i] - preds[j]).abs() < 0.8 * max_jump)).collect();
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

/// Width of the sampling bins of auto-detected curves: about 100 per panel.
pub fn sample_step(inner: &RectF) -> f64 {
    (0.01 * inner.width()).clamp(4.0, 20.0)
}

/// One point per `dx`-wide bin of a grid anchored at `x0`. The grid is the
/// panel's, so every curve of a panel is sampled at the same X values and a
/// table of all its series lines up row by row: a bin the curve covers gets a
/// point at the bin's centre (y from a least-squares line through the bin's
/// samples); a bin it only partly covers (an end, a gap of a dashed line)
/// keeps the mean of its samples.
pub fn sample_track(t: &Track, dx: f64, x0: f64) -> Vec<DataPoint> {
    let mut out = Vec::new();
    let bin_of = |x: f64| ((x - x0) / dx).floor();
    let pts = &t.pts;
    let mut i = 0;
    while i < pts.len() {
        let k = bin_of(pts[i].0);
        let mut j = i + 1;
        while j < pts.len() && bin_of(pts[j].0) == k {
            j += 1;
        }
        out.push(bin_point(&pts[i..j], x0 + (k + 0.5) * dx, dx));
        i = j;
    }
    out
}

/// The point for one bin's samples: at the bin centre `c` when they are
/// spread around it, else their mean.
fn bin_point(bin: &[(f64, f64)], c: f64, dx: f64) -> DataPoint {
    let n = bin.len() as f64;
    let mx = bin.iter().map(|p| p.0).sum::<f64>() / n;
    let my = bin.iter().map(|p| p.1).sum::<f64>() / n;
    let lo = bin.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let hi = bin.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    // Samples on both sides of the centre, filling at least half of the bin.
    if bin.len() >= 2 && lo <= c && c <= hi && hi - lo >= 0.5 * dx - 1.0 && (mx - c).abs() <= 0.25 * dx {
        let sxx: f64 = bin.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = bin.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let slope = if sxx > 1e-9 { sxy / sxx } else { 0.0 };
        DataPoint::new(c, my + slope * (c - mx))
    } else {
        DataPoint::new(mx, my)
    }
}

/// The tracks of one colour as pieces of a single curve, when they are:
/// the main (longest) track plus the substantial others (not stray
/// fragments), none of which runs alongside the main track — a piece fills a
/// gap of the main track (a spike it could not follow), while a second curve
/// of the same colour has samples in the same columns. `None` if any does.
fn pieces_of_one_curve(tracks: &[Track]) -> Option<Vec<&Track>> {
    let main = tracks.iter().max_by_key(|t| t.pts.len())?;
    let cols: std::collections::HashSet<i64> = main.pts.iter().map(|p| p.0.floor() as i64).collect();
    let mut parts = vec![main];
    for t in tracks.iter().filter(|t| !std::ptr::eq(*t, main)) {
        if t.coverage() < 0.5 * main.coverage() && (t.pts.len() as f64) < 0.25 * main.pts.len() as f64 {
            continue;
        }
        let beside = t.pts.iter().filter(|p| (-1..=1).any(|d| cols.contains(&(p.0.floor() as i64 + d)))).count();
        if beside as f64 > 0.5 * t.pts.len() as f64 {
            return None;
        }
        parts.push(t);
    }
    Some(parts)
}

/// Mask: a band of half-width `r` around a track.
fn band_mask(t: &Track, w: usize, h: usize, r: f64) -> Mask {
    Mask::from_bits(w as u32, h as u32, &band_bits(t, w, h, r))
}

/// Points of one curve made of several tracks: all their samples, binned by
/// `dx`. A bin with samples at clearly different heights (a steep flank of a
/// spike) gives one point per height, as WebPlotDigitizer's averaging window does.
fn union_points(tracks: &[&Track], dx: f64, x0: f64, sep: f64) -> Vec<DataPoint> {
    let mut all: Vec<(f64, f64)> = tracks.iter().flat_map(|t| t.pts.iter().copied()).collect();
    if all.is_empty() {
        return Vec::new();
    }
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out = Vec::new();
    let mut i = 0;
    while i < all.len() {
        let bin = ((all[i].0 - x0) / dx).floor();
        let mut j = i;
        while j < all.len() && ((all[j].0 - x0) / dx).floor() == bin {
            j += 1;
        }
        let mut b: Vec<(f64, f64)> = all[i..j].to_vec();
        b.sort_by(|p, q| p.1.partial_cmp(&q.1).unwrap());
        let mut k = 0;
        while k < b.len() {
            let mut m = k + 1;
            while m < b.len() && b[m].1 - b[m - 1].1 <= sep {
                m += 1;
            }
            out.push(bin_point(&b[k..m], x0 + (bin + 0.5) * dx, dx));
            k = m;
        }
        i = j;
    }
    out
}

/// A band of half-width `r` around a track.
fn band_bits(t: &Track, w: usize, h: usize, r: f64) -> Vec<bool> {
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
    bits
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

/// Closed boxes drawn around text inside the plot (legend or annotation
/// frames). Their edges are straight runs that would otherwise be tracked as
/// short flat series, often in a series colour. A frame is a thin top and
/// bottom run spanning the text, with matching ends joined by inked sides.
/// Grid lines run on past a cell's corners, so grid cells don't qualify.
fn text_frames(ink: &[bool], w: usize, region: &RectF, texts: &[RectF]) -> Vec<RectF> {
    let (rx0, ry0, rx1, ry1) = (region.x0 as usize, region.y0 as usize, region.x1 as usize, region.y1 as usize);
    // Horizontal ink runs (y, x0, x1), ends inclusive.
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    for y in ry0..ry1 {
        let mut x = rx0;
        while x < rx1 {
            if !ink[y * w + x] {
                x += 1;
                continue;
            }
            let s = x;
            while x < rx1 && ink[y * w + x] {
                x += 1;
            }
            if x - s >= 8 {
                runs.push((y, s, x - 1));
            }
        }
    }
    let inked = |x: usize, y: usize| (x.saturating_sub(1)..=(x + 1).min(w - 1)).any(|xx| ink[y * w + xx]);
    let side = |x: usize, y0: usize, y1: usize| (y0..=y1).filter(|&y| inked(x, y)).count() as f64 >= 0.9 * (y1 - y0 + 1) as f64;
    // Thin edge: the row just inside is mostly background (not a filled box).
    let hollow = |y: usize, x0: usize, x1: usize| ((x0 + 2)..x1.saturating_sub(1)).filter(|&x| ink[y * w + x]).count() * 2 < x1 - x0;
    let mut frames = Vec::new();
    for t in texts {
        let spans = |r: &&(usize, usize, usize)| (r.1 as f64) <= t.x0 && (r.2 as f64) >= t.x1;
        let tops: Vec<_> = runs.iter().filter(spans).filter(|r| (r.0 as f64) < t.y0).collect();
        let bots: Vec<_> = runs.iter().filter(spans).filter(|r| (r.0 as f64) > t.y1).collect();
        let mut best: Option<RectF> = None;
        for a in &tops {
            for b in &bots {
                if a.1.abs_diff(b.1) > 2 || a.2.abs_diff(b.2) > 2 {
                    continue;
                }
                let (x0, x1) = (a.1.min(b.1), a.2.max(b.2));
                let f = RectF::new(x0 as f64, a.0 as f64, (x1 + 1) as f64, (b.0 + 1) as f64);
                if f.width() > 0.6 * region.width() || f.height() > 0.6 * region.height() {
                    continue;
                }
                if best.map(|bf| f.area() >= bf.area()).unwrap_or(false) {
                    continue;
                }
                let thin = hollow(a.0 + 2, x0, x1) && hollow(b.0.saturating_sub(2), x0, x1);
                if thin && side((a.1 + b.1) / 2, a.0, b.0) && side((a.2 + b.2) / 2, a.0, b.0) {
                    best = Some(f);
                }
            }
        }
        frames.extend(best);
    }
    frames
}

/// Boxes of in-plot text lines with several words (OCR may return a title
/// as separate words). Lines of numbers alone (tick labels) are not joined.
fn text_lines(items: &[TextItem], region: &RectF) -> Vec<RectF> {
    let mut idx: Vec<usize> = (0..items.len()).filter(|&k| region.contains(items[k].bbox.center())).collect();
    idx.sort_by(|&a, &b| items[a].bbox.x0.partial_cmp(&items[b].bbox.x0).unwrap());
    let mut lines: Vec<(RectF, usize, bool)> = Vec::new();
    for k in idx {
        let b = items[k].bbox;
        let word = items[k].value.is_none();
        let same = lines.iter_mut().find(|(l, _, _)| {
            let hm = l.height().max(b.height());
            (l.center().y - b.center().y).abs() < 0.3 * hm
                && l.height() / b.height() < 1.45
                && b.height() / l.height() < 1.45
                && b.x0 - l.x1 < 1.5 * hm
        });
        match same {
            Some((l, n, w)) => {
                *l = RectF::new(l.x0.min(b.x0), l.y0.min(b.y0), l.x1.max(b.x1), l.y1.max(b.y1));
                *n += 1;
                *w |= word;
            }
            None => lines.push((b, 1, word)),
        }
    }
    lines.into_iter().filter(|(_, n, w)| *n >= 2 && *w).map(|(l, _, _)| l).collect()
}

/// Where two curves overlap or cross, their blended pixels can match a third
/// series' colour (blue over orange reads as olive green). Such fragments
/// are walled in by the other curves, while a curve's own pieces mostly border
/// the background: drop components whose outline is mostly other series.
fn drop_sandwiched(clusters: &mut [Cluster], w: usize, region: &RectF) {
    let (rx0, ry0, rx1, ry1) = (region.x0 as usize, region.y0 as usize, region.x1 as usize, region.y1 as usize);
    let h = clusters.first().map(|c| c.bits.len() / w).unwrap_or(0);
    let mut owner = vec![u16::MAX; w * h];
    for (k, c) in clusters.iter().enumerate() {
        for y in ry0..ry1 {
            for x in rx0..rx1 {
                if c.bits[y * w + x] {
                    owner[y * w + x] = k as u16;
                }
            }
        }
    }
    for (k, c) in clusters.iter_mut().enumerate() {
        let (labels, n) = label_components(&c.bits, w, h, true);
        // Per component: outline pixels owned by another series / by none.
        let mut other = vec![0usize; n as usize + 1];
        let mut free = vec![0usize; n as usize + 1];
        for y in ry0..ry1 {
            for x in rx0..rx1 {
                let l = labels[y * w + x] as usize;
                if l == 0 {
                    continue;
                }
                for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                    if nx >= w || ny >= h || c.bits[ny * w + nx] {
                        continue;
                    }
                    match owner[ny * w + nx] {
                        o if o == u16::MAX => free[l] += 1,
                        o if o as usize != k => other[l] += 1,
                        _ => {}
                    }
                }
            }
        }
        let mut count = 0;
        for (i, b) in c.bits.iter_mut().enumerate() {
            if *b {
                let l = labels[i] as usize;
                if other[l] as f64 > 0.6 * (other[l] + free[l]) as f64 {
                    *b = false;
                } else {
                    count += 1;
                }
            }
        }
        c.count = count;
    }
}

/// Drop specks of a few pixels from a cluster drawn with thick strokes (most
/// pixels at least 3 px thick both ways): they are blends and edge shades of
/// other curves, and the tracker would string them onto real fragments.
/// Thin-stroked clusters (1–2 px lines, fine dots) are left alone. Returns the
/// new pixel count.
fn drop_specks(bits: &mut [bool], w: usize, region: &RectF) -> usize {
    let (rx0, ry0, rx1, ry1) = (region.x0 as usize, region.y0 as usize, region.x1 as usize, region.y1 as usize);
    let h = bits.len() / w;
    let run = |x: usize, y: usize, dx: i64, dy: i64| {
        let mut n = 0;
        let (mut xi, mut yi) = (x as i64 + dx, y as i64 + dy);
        while n < 3 && xi >= 0 && yi >= 0 && (xi as usize) < w && (yi as usize) < h && bits[yi as usize * w + xi as usize] {
            n += 1;
            xi += dx;
            yi += dy;
        }
        n
    };
    let (mut total, mut thick) = (0usize, 0usize);
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if bits[y * w + x] {
                total += 1;
                if run(x, y, 1, 0) + run(x, y, -1, 0) >= 2 && run(x, y, 0, 1) + run(x, y, 0, -1) >= 2 {
                    thick += 1;
                }
            }
        }
    }
    if total == 0 || (thick as f64) < 0.6 * total as f64 {
        return total;
    }
    let (labels, n) = label_components(bits, w, h, true);
    let stats = component_stats(&labels, n, w);
    let mut count = 0;
    for (i, b) in bits.iter_mut().enumerate() {
        if *b {
            if stats[labels[i] as usize].area <= 4 {
                *b = false;
            } else {
                count += 1;
            }
        }
    }
    count
}

/// Median stroke thickness along a track: the length of the vertical run
/// of the cluster's pixels at each sample, on flat stretches only (steep
/// stretches have long runs whatever the stroke width).
fn track_thickness(t: &Track, bits: &[bool], w: usize, h: usize) -> f64 {
    let mut runs: Vec<usize> = Vec::new();
    for k in 1..t.pts.len() {
        let ((xa, ya), (x, y)) = (t.pts[k - 1], t.pts[k]);
        if (y - ya).abs() > 0.5 * (x - xa).abs().max(1.0) {
            continue;
        }
        let (xi, yi) = (x as usize, y as usize);
        if xi >= w || yi >= h {
            continue;
        }
        let Some(yc) = [yi, yi.saturating_sub(1), (yi + 1).min(h - 1)].into_iter().find(|&yy| bits[yy * w + xi]) else { continue };
        let mut a = yc;
        while a > 0 && bits[(a - 1) * w + xi] {
            a -= 1;
        }
        let mut b = yc;
        while b + 1 < h && bits[(b + 1) * w + xi] {
            b += 1;
        }
        runs.push(b - a + 1);
    }
    if runs.is_empty() {
        return 0.0;
    }
    runs.sort_unstable();
    runs[runs.len() / 2] as f64
}

/// Characters OCR reads from legend swatches (line samples, markers).
fn is_swatch_glyph(c: char) -> bool {
    c.is_whitespace() || "=•◦·●○■□▪▫▲△▼▽◆◇♦*+|_~—–-".contains(c)
}

/// Legend text without the swatch glyphs OCR reads around it ("= = Name",
/// "• Name", a wrapped line's trailing "-"). Returns the text and how many
/// leading characters were glyphs. A minus sign before a number is kept.
fn clean_legend_text(s: &str) -> (String, usize) {
    let chars: Vec<char> = s.chars().collect();
    let mut lead = 0;
    while lead < chars.len() && is_swatch_glyph(chars[lead]) {
        let minus = matches!(chars[lead], '-' | '–' | '—');
        if minus && chars.get(lead + 1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            break;
        }
        lead += 1;
    }
    let rest: String = chars[lead..].iter().collect();
    let rest = rest.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—'));
    (rest.to_string(), lead)
}

/// A legend entry: its name (wrapped lines joined) and its swatch pixels.
struct LegendEntry {
    name: String,
    /// Lab colours of the swatch: ink that is neither text nor grid grey.
    swatch: Vec<[f64; 3]>,
}

struct Legend {
    entries: Vec<LegendEntry>,
    /// Swatch areas and whole legend blocks, kept out of the data.
    excl: Vec<RectF>,
}

/// Legend entries and the areas they cover.
///
/// Only text with a colour swatch on its left starts an entry; a legend line
/// without one directly under an entry continues its name. Entries stacked in
/// a column form a block whose whole box is excluded. Legend-tagged text that
/// is not stacked like that (title words inside the frame, annotations) only
/// excludes its own box, never a box spanning the plot.
fn find_legend(img: &RgbaImage, items: &[TextItem], region: &RectF, bg_lab: [f64; 3]) -> Legend {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let mut idx: Vec<usize> = (0..items.len()).filter(|&k| items[k].role == TextRole::Legend).collect();
    idx.sort_by(|&a, &b| items[a].bbox.y0.partial_cmp(&items[b].bbox.y0).unwrap());
    let others: Vec<RectF> = items.iter().map(|it| it.bbox.expand(1.0)).collect();
    struct Line {
        k: usize,
        name: String,
        swatch: Vec<[f64; 3]>,
        sw_rect: Option<RectF>,
    }
    let mut lines: Vec<Line> = Vec::new();
    for &k in &idx {
        let it = &items[k];
        let th = it.bbox.height().max(6.0);
        let (name, lead) = clean_legend_text(&it.text);
        // Glyphs read from the swatch lie inside the text box: search them too.
        let glyph_w = it.bbox.width() / it.text.chars().count().max(1) as f64;
        let right = it.bbox.x0 - 1.0 + 0.8 * lead as f64 * glyph_w;
        let cy = it.bbox.center().y;
        let band = RectF::new(it.bbox.x0 - 4.0 * th, cy - 0.35 * th, right, cy + 0.35 * th).clamp_to(w as f64, h as f64);
        let mut swatch = Vec::new();
        let mut left = f64::MAX;
        for y in band.y0 as usize..band.y1.ceil() as usize {
            for x in band.x0 as usize..band.x1.ceil() as usize {
                let p = plot2csv_core::geom::Pt::new(x as f64 + 0.5, y as f64 + 0.5);
                if others.iter().enumerate().any(|(j, r)| j != k && r.contains(p)) {
                    continue;
                }
                let i = 4 * (y * w + x);
                let c = [raw[i], raw[i + 1], raw[i + 2]];
                let l = to_lab(c);
                if lab_dist(l, bg_lab) <= 22.0 || (chroma(c) < 14.0 && l[0] > 45.0) {
                    continue;
                }
                swatch.push(l);
                left = left.min(x as f64);
            }
        }
        let has = swatch.len() as f64 >= (0.5 * th).max(4.0);
        lines.push(Line {
            k,
            name,
            sw_rect: has.then(|| RectF::new(left, it.bbox.y0, right.max(left + 1.0), it.bbox.y1)),
            swatch: if has { swatch } else { Vec::new() },
        });
    }
    // Lines of one block: similar height, stacked (not side by side), overlapping in x.
    let stacked = |a: &RectF, b: &RectF| {
        let (ha, hb) = (a.height().max(1.0), b.height().max(1.0));
        let hm = ha.max(hb);
        let gap = a.y0.max(b.y0) - a.y1.min(b.y1);
        let overlap_x = a.x1.min(b.x1) - a.x0.max(b.x0);
        ha / hb < 1.45 && hb / ha < 1.45 && (a.center().y - b.center().y).abs() > 0.5 * hm && gap < 1.2 * hm && overlap_x > 0.0
    };
    let mut entries: Vec<LegendEntry> = Vec::new();
    // Legend line -> its entry (continuation lines share the entry above).
    let mut entry_of: Vec<Option<usize>> = vec![None; lines.len()];
    for li in 0..lines.len() {
        if lines[li].sw_rect.is_some() {
            entry_of[li] = Some(entries.len());
            entries.push(LegendEntry { name: lines[li].name.clone(), swatch: std::mem::take(&mut lines[li].swatch) });
            continue;
        }
        let b = items[lines[li].k].bbox;
        let above = (0..li).rev().find(|&lj| {
            let a = items[lines[lj].k].bbox;
            a.y1 <= b.y0 + 0.3 * b.height() && b.y0 - a.y1 < 0.9 * b.height() && stacked(&a, &b)
        });
        if let Some(e) = above.and_then(|lj| entry_of[lj]) {
            entry_of[li] = Some(e);
            if !lines[li].name.is_empty() {
                let n = &mut entries[e].name;
                n.push(' ');
                n.push_str(&lines[li].name);
            }
        }
    }
    let mut excl: Vec<RectF> = lines.iter().filter_map(|l| l.sw_rect.map(|r| r.expand(2.0))).collect();
    // Blocks: union-find over stacked legend lines.
    let mut parent: Vec<usize> = (0..lines.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for a in 0..lines.len() {
        for b in a + 1..lines.len() {
            if stacked(&items[lines[a].k].bbox, &items[lines[b].k].bbox) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for li in 0..lines.len() {
        let r = root(&mut parent, li);
        groups.entry(r).or_default().push(li);
    }
    for g in groups.values() {
        let mut lefts: Vec<f64> = g.iter().filter_map(|&li| lines[li].sw_rect.map(|r| r.x0)).collect();
        if g.len() < 2 || lefts.is_empty() {
            continue;
        }
        lefts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let boxes: Vec<RectF> = g.iter().map(|&li| items[lines[li].k].bbox).collect();
        let x0 = boxes.iter().map(|b| b.x0).fold(f64::MAX, f64::min).min(lefts[lefts.len() / 2]);
        let y0 = boxes.iter().map(|b| b.y0).fold(f64::MAX, f64::min);
        let x1 = boxes.iter().map(|b| b.x1).fold(f64::MIN, f64::max);
        let y1 = boxes.iter().map(|b| b.y1).fold(f64::MIN, f64::max);
        let block = RectF::new(x0, y0, x1, y1).expand(4.0);
        // A block covering much of the plot is a misread, not a legend.
        if block.intersect(region).map(|r| r.area()).unwrap_or(0.0) < 0.4 * region.area() {
            excl.push(block);
        }
    }
    Legend { entries, excl }
}

/// Grid lines: rows (or columns) mostly covered by grey or ink, unlike the
/// rows a few pixels away.
///
/// Grid lines are often lighter than the ink threshold, and a legend or title
/// box can hide part of them, so coverage counts faint grey pixels and only
/// over the part of the line that is not excluded. Rows next to a detected
/// row that are partly covered (anti-aliasing) belong to the line too.
fn grid_lines(faint: &[bool], ink: &[bool], excl: &[bool], w: usize, h: usize, region: &RectF, vertical: bool) -> Vec<bool> {
    let (rx0, ry0, rx1, ry1) = (region.x0 as usize, region.y0 as usize, region.x1 as usize, region.y1 as usize);
    let (outer, inner) = if vertical { (rx0..rx1, ry0..ry1) } else { (ry0..ry1, rx0..rx1) };
    let len = inner.len() as f64;
    let n_out = if vertical { w } else { h };
    let mut frac = vec![0.0f64; n_out];
    for o in outer.clone() {
        let (mut on, mut tot) = (0usize, 0usize);
        for k in inner.clone() {
            let i = if vertical { k * w + o } else { o * w + k };
            if excl[i] {
                continue;
            }
            tot += 1;
            if faint[i] || ink[i] {
                on += 1;
            }
        }
        if tot as f64 >= 0.25 * len {
            frac[o] = on as f64 / tot as f64;
        }
    }
    // A line, not a uniformly grey area (grainy or unevenly lit paper): the
    // rows a few pixels away are much less covered.
    let near = |o: usize, d: usize| frac.get(o + d).copied().unwrap_or(0.0).max(o.checked_sub(d).map(|i| frac[i]).unwrap_or(0.0));
    let mut out = vec![false; n_out];
    for o in outer {
        if frac[o] > 0.6 && frac[o] - near(o, 3) > 0.3 {
            out[o] = true;
            for n in [o.wrapping_sub(1), o + 1] {
                if n < n_out && frac[n] > 0.35 {
                    out[n] = true;
                }
            }
        }
    }
    out
}

/// Major/minor axis ratio of each labelled component (from second moments).
/// Markers are round or square (≈1); dashes are long and thin.
fn elongations(labels: &[u32], n: u32, w: usize) -> Vec<f64> {
    let mut m = vec![[0.0f64; 6]; n as usize + 1];
    for (i, &l) in labels.iter().enumerate() {
        if l == 0 {
            continue;
        }
        let (x, y) = ((i % w) as f64, (i / w) as f64);
        let s = &mut m[l as usize];
        s[0] += 1.0;
        s[1] += x;
        s[2] += y;
        s[3] += x * x;
        s[4] += y * y;
        s[5] += x * y;
    }
    m.iter()
        .map(|s| {
            if s[0] == 0.0 {
                return 1.0;
            }
            let (mx, my) = (s[1] / s[0], s[2] / s[0]);
            // Each pixel is a unit square: add its own variance (1/12).
            let sxx = s[3] / s[0] - mx * mx + 1.0 / 12.0;
            let syy = s[4] / s[0] - my * my + 1.0 / 12.0;
            let sxy = s[5] / s[0] - mx * my;
            let tr = sxx + syy;
            let disc = ((sxx - syy).powi(2) / 4.0 + sxy * sxy).sqrt();
            let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(1e-6));
            (l1 / l2).sqrt()
        })
        .collect()
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

    // Exclusions: text boxes (legend, labels inside the plot), legend swatches
    // and legend blocks.
    let mut excl: Vec<RectF> = Vec::new();
    for it in items {
        if region.contains(it.bbox.center()) || it.role == TextRole::Legend {
            excl.push(it.bbox.expand(2.0));
        }
    }
    // Whole lines of in-plot text (a title inside the frame): the word gaps
    // too, so the box behind the line does not count as a gap in grid lines.
    excl.extend(text_lines(items, &region).into_iter().map(|r| r.expand(2.0)));
    let legend = find_legend(img, items, &region, bg_lab);
    excl.extend(legend.excl.iter().copied());

    // Ink: far enough from the background colour. Faint: grey and visibly off
    // the background (grid lines are often lighter than the ink threshold).
    let mut ink = vec![false; w * h];
    let mut faint = vec![false; w * h];
    let mut grey = vec![false; w * h];
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * w + x;
            let c = px(x, y);
            let d = lab_dist(to_lab(c), bg_lab);
            ink[i] = d > 22.0;
            grey[i] = chroma(c) < 14.0;
            faint[i] = grey[i] && d > 6.0;
        }
    }
    // Frames around in-plot text. Any text counts, not just legend-tagged:
    // legend labels such as "1%" read as numbers and lose the legend role.
    if panel.kind != AxesKind::Bar {
        let texts: Vec<RectF> = items.iter().filter(|it| region.contains(it.bbox.center())).map(|it| it.bbox).collect();
        excl.extend(text_frames(&ink, w, &region, &texts).into_iter().map(|f| f.expand(2.0)));
    }
    let mut excluded = vec![false; w * h];
    for r in &excl {
        let Some(r) = r.intersect(&region) else { continue };
        for y in r.y0.floor() as usize..(r.y1.ceil() as usize).min(ry1) {
            for x in r.x0.floor() as usize..(r.x1.ceil() as usize).min(rx1) {
                if r.contains(plot2csv_core::geom::Pt::new(x as f64 + 0.5, y as f64 + 0.5)) {
                    excluded[y * w + x] = true;
                }
            }
        }
    }

    // Candidate ink: outside the exclusions.
    let mut cand = vec![false; w * h];
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * w + x;
            cand[i] = ink[i] && !excluded[i];
        }
    }
    // Grid lines: remove their grey ink, including the darker dots where two
    // grid lines cross. Keep grey pixels of a curve crossing the line: ink
    // continues off the grid on both sides.
    let grid_row = grid_lines(&faint, &ink, &excluded, w, h, &region, false);
    let grid_col = grid_lines(&faint, &ink, &excluded, w, h, &region, true);
    let (is_row, is_col) = (|y: usize| grid_row[y], |x: usize| grid_col[x]);
    let on_grid = |x: usize, y: usize| is_row(y) || is_col(x);
    let off_grid_ink = |x: i64, y: i64| {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && cand[y as usize * w + x as usize] && !on_grid(x as usize, y as usize)
    };
    let mut drop = Vec::new();
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * w + x;
            if !cand[i] || !grey[i] || !on_grid(x, y) {
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            let across = |dx: i64, dy: i64| {
                (2..=3).any(|k| off_grid_ink(xi - k * dx, yi - k * dy)) && (2..=3).any(|k| off_grid_ink(xi + k * dx, yi + k * dy))
            };
            let keep = (!is_row(y) || across(0, 1)) && (!is_col(x) || across(1, 0));
            if !keep {
                drop.push(i);
            }
        }
    }
    for i in drop {
        cand[i] = false;
    }
    let n_cand = cand.iter().filter(|b| **b).count();

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
        colors.iter().map(|&c| Cluster { color: c, lab: to_lab(c), fade: fade_path(c, bg), bits: vec![false; w * h], count: 0 }).collect();
    if clusters.is_empty() {
        return SeriesResult { datasets: vec![], note: "No data-coloured pixels found inside the plot area.".into() };
    }
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            let i = y * w + x;
            if !cand[i] {
                continue;
            }
            // Match against each colour's anti-aliased shades, not the colour
            // alone: a light edge pixel of one curve can be nearer another,
            // lighter series colour (or grey) than its own, and those strays
            // become a dotted copy of the curve in the wrong colour.
            let l = to_lab(px(x, y));
            let mut best = (f64::MAX, 0usize);
            for (k, c) in clusters.iter().enumerate() {
                let d = fade_dist(l, c);
                if d < best.0 {
                    best = (d, k);
                }
            }
            // Every fade ends at the background, so faint pixels are near all
            // of them: assign only if clearly nearer the cluster's shades.
            if best.0 < 0.6 * lab_dist(l, bg_lab) {
                clusters[best.1].bits[i] = true;
                clusters[best.1].count += 1;
            }
        }
    }
    drop_sandwiched(&mut clusters, w, &region);
    for c in clusters.iter_mut() {
        c.count = drop_specks(&mut c.bits, w, &region);
    }
    clusters.retain(|c| c.count >= 25);

    // Legend names: each swatch pixel votes for the cluster whose shades it
    // matches (the same test as the data pixels); a cluster takes the name of
    // the entry with the most votes for it.
    let mut name_votes: Vec<Option<(usize, usize, String)>> = vec![None; clusters.len()];
    // How many legend entries show each cluster's colour.
    let mut n_entries = vec![0usize; clusters.len()];
    for (ei, e) in legend.entries.iter().enumerate() {
        let mut votes = vec![0usize; clusters.len()];
        for &l in &e.swatch {
            let best = clusters.iter().enumerate().map(|(k, c)| (fade_dist(l, c), k)).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if let Some((d, k)) = best {
                if d < 0.6 * lab_dist(l, bg_lab) {
                    votes[k] += 1;
                }
            }
        }
        let Some((k, &n)) = votes.iter().enumerate().max_by_key(|v| *v.1) else { continue };
        if n >= 3 && n as f64 >= 0.3 * e.swatch.len() as f64 {
            n_entries[k] += 1;
            if !e.name.is_empty() && name_votes[k].as_ref().map(|v| n > v.0).unwrap_or(true) {
                name_votes[k] = Some((n, ei, e.name.clone()));
            }
        }
    }
    // Grey ink lying along grid rows (fragments and crossings the grid
    // removal missed) is grid, not data.
    let near_grid_row = |y: f64| {
        let y = y.floor() as i64;
        (y - 1..=y + 1).any(|r| r >= 0 && (r as usize) < grid_row.len() && grid_row[r as usize])
    };
    let along_grid = |t: &Track| t.pts.iter().filter(|p| near_grid_row(p.1)).count() as f64 > 0.6 * t.pts.len() as f64;

    // Sampling grid shared by every curve of the panel.
    let dx = sample_step(&inner);
    let grid_x0 = inner.x0;
    let max_jump = (0.03 * inner.height()).max(6.0);
    let max_gap = (0.03 * inner.width()).max(8.0);
    let mut datasets: Vec<Dataset> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    // Legend order first, then the rest by pixel count (the cluster order).
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    order.sort_by_key(|&k| name_votes[k].as_ref().map(|v| v.1).unwrap_or(usize::MAX));
    for ci in order {
        let c = &clusters[ci];
        let bm = Bitmap { bits: &c.bits, width: w, height: h };
        let base_name = name_votes[ci].as_ref().map(|v| v.2.clone()).unwrap_or_else(|| color_name(c.color).to_string());
        let greyish = chroma(c.color) < 14.0;
        // A grey cluster made of what is left on the grid lines.
        if greyish {
            let on = (ry0..ry1).flat_map(|y| (rx0..rx1).map(move |x| (x, y))).filter(|&(x, y)| c.bits[y * w + x] && on_grid(x, y)).count();
            if on as f64 > 0.5 * c.count as f64 {
                continue;
            }
        }
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
        let elong = elongations(&labels, n, w);
        let small_compact: Vec<_> = stats
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, s)| {
                let (bw, bh) = (s.width() as f64, s.height() as f64);
                let fill = s.area as f64 / (bw * bh);
                s.area >= 6 && bw < 0.06 * inner.width() && bh < 0.06 * inner.height() && bw / bh > 0.4 && bw / bh < 2.5 && fill > 0.3
            })
            .collect();
        let compact_area: usize = small_compact.iter().map(|(_, s)| s.area).sum();
        // Dashes of a dashed line are small too, but long and thin; markers
        // (circles, squares, triangles, crosses) are about as wide as long.
        let mut el: Vec<f64> = small_compact.iter().map(|(k, _)| elong[*k]).collect();
        el.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let round = el.get(el.len() / 2).map(|e| *e < 1.7).unwrap_or(false);
        let is_markers = small_compact.len() >= 4 && compact_area as f64 > 0.7 * c.count as f64 && round;
        let small_compact: Vec<_> = small_compact.into_iter().map(|(_, s)| s).collect();

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
            .filter(|t| t.span() >= 0.05 * region.width() && t.pts.len() >= 6 && !(greyish && along_grid(t)))
            .collect();
        let tracks: Vec<Track> = join_disjoint(tracks)
            .into_iter()
            .filter(|t| t.span() >= 0.12 * region.width() && t.pts.len() >= 10 && !is_grid_like(t, c.color) && !(greyish && along_grid(t)))
            .collect();
        // Thin traces beside a thick curve: where curves of other colours
        // meet, resampling leaves 1-px edges whose colour can match this one.
        let tracks: Vec<Track> = match tracks.iter().max_by_key(|t| t.pts.len()) {
            Some(main) if track_thickness(main, &c.bits, w, h) >= 3.0 => {
                let main_pts = main.pts.len();
                tracks.iter().filter(|t| t.pts.len() == main_pts || track_thickness(t, &c.bits, w, h) > 1.5).cloned().collect()
            }
            _ => tracks,
        };
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
                Some(t) => sample_track(t, dx, grid_x0),
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
        } else if let Some(parts) = (n_entries[ci] == 1).then(|| pieces_of_one_curve(&tracks)).flatten() {
            // The legend shows one series in this colour and the tracks are
            // pieces of one curve (split at spikes and crossings).
            let mut ds = Dataset::new(base_name.clone());
            ds.color = c.color;
            let n_pts: usize = parts.iter().map(|t| t.pts.len()).sum();
            let x0 = parts.iter().map(|t| t.first().0).fold(f64::MAX, f64::min);
            let x1 = parts.iter().map(|t| t.last().0).fold(f64::MIN, f64::max);
            ds.style = if n_pts as f64 / (x1 - x0 + 1.0) < 0.85 { SeriesStyle::Dashed } else { SeriesStyle::Line };
            ds.points = union_points(&parts, dx, grid_x0, 0.5 * max_jump);
            let mut bits = vec![false; w * h];
            for t in &parts {
                for (b, m) in bits.iter_mut().zip(band_bits(t, w, h, max_jump.max(5.0))) {
                    *b |= m;
                }
            }
            auto.mask = Some(Mask::from_bits(w as u32, h as u32, &bits));
            ds.auto = auto;
            notes.push(format!("{}: {} points", ds.name, ds.points.len()));
            datasets.push(ds);
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
                ds.points = sample_track(t, dx, grid_x0);
                let mut a = auto.clone();
                a.mask = Some(band_mask(t, w, h, (0.5 * max_jump).max(4.0)));
                ds.auto = a;
                notes.push(format!("{}: {} points", ds.name, ds.points.len()));
                datasets.push(ds);
            }
        }
    }
    let note = if datasets.is_empty() {
        "No series found automatically; add points by hand or adjust the colour settings.".to_string()
    } else {
        format!("Found {} series — {}.", datasets.len(), notes.join("; "))
    };
    SeriesResult { datasets, note }
}

/// Attach category labels (text under each bar) to bar points.
fn label_bars(mut pts: Vec<DataPoint>, items: &[TextItem], inner: &RectF) -> Vec<DataPoint> {
    let cats = CategoryLabels::new(items, inner);
    for p in pts.iter_mut() {
        p.label = cats.at(p.x);
    }
    pts
}

/// The category labels of a bar chart: the row of text right under the plot.
/// Axis titles, numbers and the other rows are not categories, so a bar never
/// takes the X title ("Datasets") when it is closer than the bar's own label.
pub struct CategoryLabels<'a> {
    row: Vec<&'a TextItem>,
    /// Typical distance between neighbouring labels.
    spacing: f64,
}

impl<'a> CategoryLabels<'a> {
    pub fn new(items: &'a [TextItem], inner: &RectF) -> Self {
        let margin = 0.05 * inner.width();
        let below: Vec<&TextItem> = items
            .iter()
            .filter(|it| {
                let c = it.bbox.center();
                c.y > inner.y1
                    && c.x > inner.x0 - margin
                    && c.x < inner.x1 + margin
                    && it.value.is_none()
                    && !it.vertical
                    && !it.text.trim().is_empty()
                    && matches!(it.role, TextRole::XTick | TextRole::Other)
            })
            .collect();
        // Labels classified as X ticks win; otherwise the row nearest the axis.
        let ticks: Vec<&TextItem> = below.iter().copied().filter(|it| it.role == TextRole::XTick).collect();
        let pool = if ticks.is_empty() { below } else { ticks };
        let row: Vec<&TextItem> = match pool.iter().min_by(|a, b| a.bbox.center().y.total_cmp(&b.bbox.center().y)) {
            Some(top) => {
                let (y, h) = (top.bbox.center().y, top.bbox.height().max(6.0));
                pool.iter().copied().filter(|it| (it.bbox.center().y - y).abs() < 0.6 * h.max(it.bbox.height())).collect()
            }
            None => Vec::new(),
        };
        let mut xs: Vec<f64> = row.iter().map(|it| it.bbox.center().x).collect();
        xs.sort_by(f64::total_cmp);
        let mut gaps: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 1.0).collect();
        gaps.sort_by(f64::total_cmp);
        let spacing = gaps.get(gaps.len() / 2).copied().unwrap_or(inner.width());
        Self { row, spacing }
    }

    /// Label of the category a bar at pixel `x` belongs to: the nearest label,
    /// if the bar lies within its group (about half the label spacing).
    pub fn at(&self, x: f64) -> Option<String> {
        let (d, it) = self.row.iter().map(|it| ((it.bbox.center().x - x).abs(), *it)).min_by(|a, b| a.0.total_cmp(&b.0))?;
        let reach = (0.5 * it.bbox.width()).max(0.6 * self.spacing).max(20.0);
        (d <= reach).then(|| it.text.trim().to_string())
    }
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

    /// Anti-aliased 2 px curve: rows partly covered by the stroke get a blend
    /// of the colour and white, as a rasteriser's box filter produces.
    fn draw_aa(img: &mut RgbaImage, f: impl Fn(f64) -> f64, x0: u32, x1: u32, col: [u8; 3]) {
        for x in x0..x1 {
            let yc = f(x as f64);
            let (top, bot) = (yc - 1.0, yc + 1.0);
            for row in (top.floor() as u32)..=(bot.floor() as u32) {
                let cover = (bot.min(row as f64 + 1.0) - top.max(row as f64)).clamp(0.0, 1.0);
                if cover <= 0.0 {
                    continue;
                }
                let p = img.get_pixel(x, row).0;
                let mix = |k: usize| (cover * col[k] as f64 + (1.0 - cover) * p[k] as f64).round() as u8;
                img.put_pixel(x, row, Rgba([mix(0), mix(1), mix(2), 255]));
            }
        }
    }

    #[test]
    fn antialiased_edges_stay_with_their_curve() {
        // Excel's blue and purple: purple's light edge pixels are nearer the
        // (lighter) blue than purple itself, and used to form a dotted "Blue 2".
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        draw_aa(&mut img, |x| 330.0 - 250.0 * (-(x - 60.0) / 120.0).exp(), 60, 560, [74, 126, 187]);
        draw_aa(&mut img, |x| 60.0 + 0.0004 * (x - 60.0).powi(2), 60, 560, [125, 96, 160]);
        let r = detect_series(&img, &panel(inner), &[], None);
        let names: Vec<&str> = r.datasets.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Blue", "Purple"], "{}", r.note);
    }

    fn text(bbox: RectF, t: &str, role: TextRole) -> TextItem {
        TextItem { bbox, text: t.into(), conf: 0.95, role, value: None, manual: false, vertical: false }
    }

    #[test]
    fn grid_crossings_and_title_words_are_not_series() {
        // A semi-transparent grid: single lines are lighter than the ink
        // threshold, but where two cross they are dark enough to be ink. A
        // title inside the frame is OCR'd as words that get the legend role;
        // with the real legend they used to make one box over half the plot.
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        for y in 30..370u32 {
            for x in 50..570u32 {
                let n = (x % 20 == 0) as i32 + (y % 20 == 0) as i32;
                let v = (255.0 * 0.78f64.powi(n)) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let white = |img: &mut RgbaImage, r: (u32, u32, u32, u32)| {
            for y in r.1..r.3 {
                for x in r.0..r.2 {
                    img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
        };
        white(&mut img, (150, 35, 450, 66));
        white(&mut img, (436, 96, 566, 206));
        let orange = |x: f64| 220.0 - 140.0 * (-((x - 250.0) / 80.0).powi(2)).exp();
        let blue = |x: f64| 330.0 - 0.15 * (x - 60.0);
        draw_curve(&mut img, orange, 60, 430, [230, 120, 30], None, 3);
        draw_curve(&mut img, blue, 60, 560, [40, 90, 200], None, 3);
        draw_curve(&mut img, |_| 110.0, 442, 466, [230, 120, 30], None, 3);
        draw_curve(&mut img, |_| 140.0, 442, 466, [40, 90, 200], None, 3);
        let items = vec![
            text(RectF::new(160.0, 40.0, 250.0, 60.0), "Response", TextRole::Legend),
            text(RectF::new(260.0, 40.0, 350.0, 60.0), "Spectrum", TextRole::Legend),
            text(RectF::new(360.0, 40.0, 420.0, 60.0), "Title", TextRole::Legend),
            text(RectF::new(472.0, 104.0, 560.0, 118.0), "= = Orange model", TextRole::Legend),
            text(RectF::new(478.0, 120.0, 540.0, 134.0), "Vs30=760m/s", TextRole::Legend),
            text(RectF::new(472.0, 134.0, 560.0, 148.0), "• Blue site -", TextRole::Legend),
            text(RectF::new(478.0, 150.0, 530.0, 164.0), "profile 3", TextRole::Legend),
        ];
        let r = detect_series(&img, &panel(inner), &items, None);
        let names: Vec<&str> = r.datasets.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Orange model Vs30=760m/s", "Blue site profile 3"], "{}", r.note);
        for (d, f) in r.datasets.iter().zip([&orange as &dyn Fn(f64) -> f64, &blue]) {
            let (lo, hi) = (d.points.first().unwrap().x, d.points.last().unwrap().x);
            assert!(lo < 80.0 && hi > if d.name.starts_with("Orange") { 410.0 } else { 540.0 }, "{} spans {lo}..{hi}", d.name);
            for p in &d.points {
                assert!((p.y - f(p.x) - 1.0).abs() < 4.0, "{}: {p:?} is off the curve", d.name);
            }
        }
    }

    #[test]
    fn black_curve_on_uneven_paper_survives() {
        // A photographed chart: the paper darkens across the frame and is
        // grainy, so faint grey is everywhere. That is not a grid, and grey
        // ink (a black curve) must stay.
        let mut img = RgbaImage::from_fn(600, 400, |x, y| {
            let n = ((x * 7919 + y * 104729) % 9) as f64 - 4.0;
            let v = (252.0 - 8.0 * x as f64 / 600.0 - 50.0 * y as f64 / 400.0 + n) as u8;
            Rgba([v, v, v.saturating_sub(3), 255])
        });
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        let f = |x: f64| 200.0 + 150.0 * (x / 70.0).sin();
        draw_curve(&mut img, f, 60, 560, [25, 25, 25], None, 2);
        let r = detect_series(&img, &panel(inner), &[], None);
        assert_eq!(r.datasets.len(), 1, "{}", r.note);
        let d = &r.datasets[0];
        assert!(d.points.first().unwrap().x < 80.0 && d.points.last().unwrap().x > 540.0, "{}", r.note);
        assert!(d.points.iter().all(|p| (p.y - f(p.x) - 0.5).abs() < 3.0), "{}", r.note);
        // The crests, on the palest paper, are there too.
        assert!(d.points.iter().filter(|p| p.y < 70.0).count() >= 6, "{}", r.note);
    }

    #[test]
    fn same_colour_curves_under_one_legend_entry_stay_apart() {
        // One legend entry, two curves of its colour over the whole range
        // (a family of runs): the tracks run side by side, so they are not
        // pieces of one curve and are not merged into one interleaved series.
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        let col = [40, 90, 200];
        draw_curve(&mut img, |x| 120.0 + 0.3 * x, 60, 430, col, None, 3);
        draw_curve(&mut img, |x| 330.0 - 0.3 * x, 60, 430, col, None, 3);
        draw_curve(&mut img, |_| 60.0, 470, 500, col, None, 3);
        let items = vec![text(RectF::new(506.0, 53.0, 560.0, 67.0), "Runs", TextRole::Legend)];
        let r = detect_series(&img, &panel(inner), &items, None);
        assert_eq!(r.datasets.len(), 2, "{}", r.note);
        assert!(r.datasets.iter().all(|d| d.name.starts_with("Runs")), "{}", r.note);
    }

    #[test]
    fn thick_dashes_are_not_markers() {
        // Short, thick dashes on a slope have square-ish boxes like markers,
        // but each dash is long and thin.
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        draw_curve(&mut img, |x| 320.0 - 0.4 * x, 60, 560, [230, 20, 20], Some((10, 6)), 4);
        let r = detect_series(&img, &panel(inner), &[], None);
        assert_eq!(r.datasets.len(), 1, "{}", r.note);
        assert_eq!(r.datasets[0].style, SeriesStyle::Dashed, "{}", r.note);
        assert!(r.datasets[0].points.len() > 20, "{}", r.note);
    }

    #[test]
    fn legend_text_loses_swatch_glyphs() {
        assert_eq!(clean_legend_text("= = Chiou & Youngs (2008)"), ("Chiou & Youngs (2008)".into(), 4));
        assert_eq!(clean_legend_text("• SHAKE2000 adjusted motions"), ("SHAKE2000 adjusted motions".into(), 2));
        assert_eq!(clean_legend_text("SHAKE2000 - artificial motions -"), ("SHAKE2000 - artificial motions".into(), 0));
        assert_eq!(clean_legend_text("-5 dB"), ("-5 dB".into(), 0));
    }

    #[test]
    fn coloured_legend_frame_is_not_a_series() {
        // A legend box framed in the series colour, its labels read as numbers
        // ("1%") so they are not tagged as legend text.
        let mut img = RgbaImage::from_pixel(600, 400, Rgba([255, 255, 255, 255]));
        let inner = RectF::new(50.0, 30.0, 570.0, 370.0);
        let blue = [74, 126, 187];
        draw_curve(&mut img, |x| 330.0 - 0.5 * x, 60, 400, blue, None, 2);
        let frame = (440u32, 60u32, 540u32, 140u32);
        for x in frame.0..=frame.2 {
            img.put_pixel(x, frame.1, Rgba([79, 129, 189, 255]));
            img.put_pixel(x, frame.3, Rgba([79, 129, 189, 255]));
        }
        for y in frame.1..=frame.3 {
            img.put_pixel(frame.0, y, Rgba([79, 129, 189, 255]));
            img.put_pixel(frame.2, y, Rgba([79, 129, 189, 255]));
        }
        draw_curve(&mut img, |_| 79.0, 455, 480, blue, None, 2);
        let label = TextItem {
            bbox: RectF::new(488.0, 74.0, 510.0, 84.0),
            text: "1%".into(),
            conf: 0.9,
            role: TextRole::Other,
            value: Some(1.0),
            manual: false,
            vertical: false,
        };
        let r = detect_series(&img, &panel(inner), &[label], None);
        assert_eq!(r.datasets.len(), 1, "{}", r.note);
        assert!(r.datasets[0].points.iter().all(|p| p.x < 410.0), "{}", r.note);
    }

    /// Curves of one panel are sampled on one X grid: the same X values for
    /// every series, at bin centres, with y on the curve.
    #[test]
    fn curves_share_the_sampling_grid() {
        let line = |x0: usize, x1: usize, f: &dyn Fn(f64) -> f64| Track {
            pts: (x0..x1).map(|x| (x as f64 + 0.5, f(x as f64 + 0.5))).collect(),
            slope: 0.0,
        };
        let a = line(106, 500, &|x| 0.3 * x + 10.0);
        let b = line(117, 480, &|x| 200.0 - 0.1 * x);
        let (dx, x0) = (8.0, 100.0);
        let (pa, pb) = (sample_track(&a, dx, x0), sample_track(&b, dx, x0));
        // Interior points sit at bin centres x0 + (k + ½)·dx, exactly on the line.
        for p in pa[1..pa.len() - 1].iter() {
            let k = (p.x - x0) / dx - 0.5;
            assert!((k - k.round()).abs() < 1e-9, "x {} off the grid", p.x);
            assert!((p.y - (0.3 * p.x + 10.0)).abs() < 1e-9);
        }
        let xa: std::collections::HashSet<i64> = pa[1..pa.len() - 1].iter().map(|p| (p.x * 1e6).round() as i64).collect();
        let shared = pb[1..pb.len() - 1].iter().filter(|p| xa.contains(&((p.x * 1e6).round() as i64))).count();
        assert_eq!(shared, pb.len() - 2, "every interior X of b is an X of a");
        // A partly covered end bin (two samples of eight columns) keeps their mean.
        assert!((pa[0].x - 107.0).abs() < 1e-9 && (pa[0].y - (0.3 * 107.0 + 10.0)).abs() < 1e-9, "{:?}", pa[0]);
    }

    /// Grouped bars (bar_input.png): the X title "Datasets" sits under the gap
    /// between DS3 and DS4, closer to DS3's last bar than the DS3 label is.
    #[test]
    fn bar_labels_skip_the_axis_title() {
        let inner = RectF::new(120.0, 95.0, 965.0, 595.0);
        let text = |x0: f64, y0: f64, x1: f64, y1: f64, t: &str, role: TextRole| TextItem {
            bbox: RectF::new(x0, y0, x1, y1),
            text: t.into(),
            conf: 0.9,
            role,
            value: None,
            manual: false,
            vertical: false,
        };
        let mut items: Vec<TextItem> = (0..6)
            .map(|k| {
                let cx = 192.0 + 140.0 * k as f64;
                text(cx - 28.0, 612.0, cx + 28.0, 645.0, &format!("DS{}", k + 1), TextRole::XTick)
            })
            .collect();
        items.push(text(448.0, 672.0, 640.0, 708.0, "Datasets", TextRole::XTitle));
        // The bars of group DS3 span x = 422..512 around its label at 472.
        let bars: Vec<DataPoint> = [422.0, 452.0, 487.0, 507.0, 512.0].iter().map(|&x| DataPoint::new(x, 300.0)).collect();
        let labelled = label_bars(bars.clone(), &items, &inner);
        assert!(labelled.iter().all(|p| p.label.as_deref() == Some("DS3")), "{:?}", labelled);
        // Also when the title was not recognised as one.
        items.last_mut().unwrap().role = TextRole::Other;
        let labelled = label_bars(bars, &items, &inner);
        assert!(labelled.iter().all(|p| p.label.as_deref() == Some("DS3")), "{:?}", labelled);
        // First and last groups reach out to their outermost bars.
        let cats = CategoryLabels::new(&items, &inner);
        assert_eq!(cats.at(135.0).as_deref(), Some("DS1"));
        assert_eq!(cats.at(955.0).as_deref(), Some("DS6"));
    }
}
