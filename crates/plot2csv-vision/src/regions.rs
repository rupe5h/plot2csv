//! Plot-area (panel) detection: framed boxes and L-shaped axes, each with an
//! outer box that includes tick labels and titles.

use image::GrayImage;
use plot2csv_core::axes::AxesKind;
use plot2csv_core::geom::{Pt, RectF};

use crate::lines::{detect_ink, hough_segments, stroke_contrast, Ink, Segment};
use crate::raster::{component_stats, downscale_to, label_components};

/// Minimum two-sided contrast (grey levels) for a line to count as a drawn
/// axis or frame. Step edges (photographed page borders, bar sides) score ~0;
/// even light grid lines score 30+.
const MIN_STROKE_CONTRAST: f64 = 12.0;

/// Keep drawn lines; drop step edges such as the border of a photographed page,
/// which would otherwise form a large L-shaped "axes" around the real plot.
fn strokes_only(g: &GrayImage, segs: Vec<Segment>) -> Vec<Segment> {
    segs.into_iter().filter(|s| stroke_contrast(g, s).is_none_or(|c| c >= MIN_STROKE_CONTRAST)).collect()
}

#[derive(Clone, Debug)]
pub struct PanelGuess {
    pub inner: RectF,
    pub outer: RectF,
    pub kind: AxesKind,
    /// "frame" or "axes"
    pub shape: &'static str,
    pub score: f64,
}

pub struct RegionResult {
    pub panels: Vec<PanelGuess>,
    pub note: String,
}

const WORK_SIDE: u32 = 1200;

fn horiz_segments(ink: &Ink, min_len: f64) -> Vec<Segment> {
    hough_segments(ink, 0.0, 1.5, 0.1, min_len, 60)
}

fn vert_segments(ink: &Ink, min_len: f64) -> Vec<Segment> {
    hough_segments(ink, 90.0, 1.5, 0.1, min_len, 60)
}

/// Normalise a near-horizontal segment as (x0, x1, y).
fn hseg(s: &Segment) -> (f64, f64, f64) {
    let (a, b) = if s.a.x <= s.b.x { (s.a, s.b) } else { (s.b, s.a) };
    (a.x, b.x, (a.y + b.y) / 2.0)
}

/// Normalise a near-vertical segment as (y0, y1, x).
fn vseg(s: &Segment) -> (f64, f64, f64) {
    let (a, b) = if s.a.y <= s.b.y { (s.a, s.b) } else { (s.b, s.a) };
    (a.y, b.y, (a.x + b.x) / 2.0)
}

pub fn detect_panels(src: &GrayImage) -> RegionResult {
    let (g, s) = downscale_to(src, WORK_SIDE);
    let s = s as f64;
    let ink = detect_ink(&g, 12, 22.0);
    let (w, h) = (g.width() as f64, g.height() as f64);
    let hs: Vec<(f64, f64, f64)> = strokes_only(&g, horiz_segments(&ink, 0.12 * w)).iter().map(hseg).collect();
    let vs: Vec<(f64, f64, f64)> = strokes_only(&g, vert_segments(&ink, 0.10 * h)).iter().map(vseg).collect();
    let tol = 0.025 * w.max(h);
    let mut cands: Vec<PanelGuess> = Vec::new();

    // L-shapes: a vertical axis whose bottom meets the left end of a horizontal axis.
    for &(hx0, hx1, hy) in &hs {
        for &(vy0, vy1, vx) in &vs {
            if (vy1 - hy).abs() < tol && (vx - hx0).abs() < tol {
                let inner = RectF::new(vx, vy0, hx1, hy);
                if inner.width() > 0.1 * w && inner.height() > 0.1 * h {
                    cands.push(PanelGuess { inner, outer: inner, kind: AxesKind::Xy, shape: "axes", score: inner.area() });
                }
            }
        }
    }
    // Frames: top + bottom horizontals spanning between left + right verticals.
    for (i, &(ax0, ax1, ay)) in hs.iter().enumerate() {
        for &(bx0, bx1, by) in hs.iter().skip(i + 1) {
            let (top, bot) = if ay < by { ((ax0, ax1, ay), (bx0, bx1, by)) } else { ((bx0, bx1, by), (ax0, ax1, ay)) };
            if bot.2 - top.2 < 0.1 * h || (top.0 - bot.0).abs() > tol || (top.1 - bot.1).abs() > tol {
                continue;
            }
            let left = vs.iter().any(|&(y0, y1, x)| (x - top.0).abs() < tol && y0 < top.2 + tol && y1 > bot.2 - tol);
            let right = vs.iter().any(|&(y0, y1, x)| (x - top.1).abs() < tol && y0 < top.2 + tol && y1 > bot.2 - tol);
            if left && right {
                let inner = RectF::new((top.0 + bot.0) / 2.0, top.2, (top.1 + bot.1) / 2.0, bot.2);
                cands.push(PanelGuess { inner, outer: inner, kind: AxesKind::Xy, shape: "frame", score: inner.area() * 1.2 });
            }
        }
    }
    // Largest first; drop candidates overlapping an accepted one. A candidate
    // that contains two or more disjoint large candidates is a figure border.
    // So is one around a single large candidate inset on all four sides (an
    // Excel chart-area border, a figure frame): its margin holds the tick
    // labels. Grid-line strips of a real frame share its sides, so they don't
    // count, and insets and legend boxes are too small.
    cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let contains = |a: &RectF, b: &RectF| b.intersect(a).map(|r| r.area() / b.area().max(1e-9)).unwrap_or(0.0) > 0.9;
    let min_inset = (0.008 * w.max(h)).max(4.0);
    let inset_in =
        |outer: &RectF, d: &RectF| [d.x0 - outer.x0, d.y0 - outer.y0, outer.x1 - d.x1, outer.y1 - d.y1].iter().all(|m| *m > min_inset);
    let mut accepted: Vec<PanelGuess> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let inside: Vec<&PanelGuess> =
            cands.iter().skip(i + 1).filter(|d| contains(&c.inner, &d.inner) && d.inner.area() > 0.12 * c.inner.area()).collect();
        let mut disjoint = 0;
        for (k, d) in inside.iter().enumerate() {
            if inside.iter().take(k).all(|e| e.inner.intersect(&d.inner).is_none()) {
                disjoint += 1;
            }
        }
        if disjoint >= 2 {
            continue;
        }
        if inside.iter().any(|d| d.inner.area() > 0.35 * c.inner.area() && inset_in(&c.inner, &d.inner)) {
            continue;
        }
        let overlaps = accepted.iter().any(|a| a.inner.iou(&c.inner) > 0.1 || contains(&a.inner, &c.inner) || contains(&c.inner, &a.inner));
        if !overlaps && c.inner.area() > 0.03 * w * h {
            accepted.push(c.clone());
        }
    }
    let mut note;
    if accepted.is_empty() {
        // Fall back to the bounding box of the ink.
        let inner = ink_bbox(&ink).unwrap_or(RectF::new(0.0, 0.0, w, h));
        accepted.push(PanelGuess { inner, outer: RectF::new(0.0, 0.0, w, h), kind: AxesKind::Xy, shape: "image", score: 0.0 });
        note = "No axes or frame found; using the whole image. Adjust the plot area if needed.".to_string();
    } else {
        note = format!(
            "Found {} plot area{} ({}).",
            accepted.len(),
            if accepted.len() == 1 { "" } else { "s" },
            accepted.iter().map(|p| p.shape).collect::<Vec<_>>().join(", ")
        );
    }
    // Outer boxes from ink around each inner box, not entering other panels.
    let inners: Vec<RectF> = accepted.iter().map(|p| p.inner).collect();
    for (i, p) in accepted.iter_mut().enumerate() {
        if p.shape != "image" {
            p.outer = grow_outer(&ink, &p.inner, &inners, i);
        }
        if is_bar_chart(&g, &ink, &p.inner) {
            p.kind = AxesKind::Bar;
        }
    }
    if accepted.iter().any(|p| p.kind == AxesKind::Bar) {
        note.push_str(" Looks like a bar chart.");
    }
    // Reading order.
    accepted.sort_by(|a, b| {
        let ra = (a.inner.y0 / (0.2 * h)).floor();
        let rb = (b.inner.y0 / (0.2 * h)).floor();
        ra.partial_cmp(&rb).unwrap().then(a.inner.x0.partial_cmp(&b.inner.x0).unwrap())
    });
    // Back to source px.
    let back = |r: RectF| RectF::new(r.x0 / s, r.y0 / s, r.x1 / s, r.y1 / s);
    for p in accepted.iter_mut() {
        p.inner = back(p.inner);
        p.outer = back(p.outer);
    }
    RegionResult { panels: accepted, note }
}

fn ink_bbox(ink: &Ink) -> Option<RectF> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, b) in ink.bits.iter().enumerate() {
        if *b {
            let (x, y) = (i % ink.w, i / ink.w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 < x1 && y0 < y1).then(|| RectF::new(x0 as f64, y0 as f64, x1 as f64 + 1.0, y1 as f64 + 1.0))
}

/// Grow outward from the inner box while ink continues, stopping at a gap or another panel.
fn grow_outer(ink: &Ink, inner: &RectF, others: &[RectF], me: usize) -> RectF {
    let (w, h) = (ink.w as f64, ink.h as f64);
    let gap = (0.035 * w.max(h)).max(14.0);
    let max_l = 0.30 * inner.width() + 60.0;
    let max_b = 0.30 * inner.height() + 60.0;
    let max_t = 0.20 * inner.height() + 40.0;
    let max_r = 0.15 * inner.width() + 30.0;
    let blocked = |x: f64, y: f64| others.iter().enumerate().any(|(i, o)| i != me && o.contains(Pt::new(x, y)));
    let col_has_ink = |x: f64, y0: f64, y1: f64| -> bool {
        let xi = x as i64;
        (y0.max(0.0) as i64..y1.min(h) as i64).any(|y| ink.get(xi, y) && !blocked(x, y as f64))
    };
    let row_has_ink = |y: f64, x0: f64, x1: f64| -> bool {
        let yi = y as i64;
        (x0.max(0.0) as i64..x1.min(w) as i64).any(|x| ink.get(x, yi) && !blocked(x as f64, y))
    };
    let scan = |start: f64, dir: f64, limit: f64, has: &dyn Fn(f64) -> bool| -> f64 {
        let mut last = start;
        let mut t = start;
        let end = (start + dir * limit).max(0.0);
        loop {
            t += dir;
            if (dir < 0.0 && (t < 0.0 || t < end)) || (dir > 0.0 && t > end) {
                break;
            }
            if has(t) {
                last = t;
            } else if (t - last).abs() > gap {
                break;
            }
        }
        last
    };
    let (y0, y1) = (inner.y0 - 4.0, inner.y1 + 4.0);
    let left = scan(inner.x0 - 3.0, -1.0, max_l, &|x| x >= 0.0 && col_has_ink(x, y0, y1));
    let right = scan(inner.x1 + 3.0, 1.0, max_r, &|x| x < w && col_has_ink(x, y0, y1));
    let (x0, x1) = (left - 2.0, right + 2.0);
    let bottom = scan(inner.y1 + 3.0, 1.0, max_b, &|y| y < h && row_has_ink(y, x0, x1));
    let top = scan(inner.y0 - 3.0, -1.0, max_t, &|y| y >= 0.0 && row_has_ink(y, x0, x1));
    RectF::new((left - 4.0).max(0.0), (top - 4.0).max(0.0), (right + 4.0).min(w), (bottom + 4.0).min(h))
}

/// Several solid filled rectangles standing inside the plot suggest a bar chart.
fn is_bar_chart(g: &GrayImage, _ink: &Ink, inner: &RectF) -> bool {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let (x0, y0) = ((inner.x0 + 3.0).max(0.0) as usize, (inner.y0 + 3.0).max(0.0) as usize);
    let (x1, y1) = ((inner.x1 - 3.0).min(w as f64) as usize, (inner.y1 - 3.0).min(h as f64) as usize);
    if x1 <= x0 + 10 || y1 <= y0 + 10 {
        return false;
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    // Non-white, filled pixels.
    let mut mask = vec![false; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            mask[y * cw + x] = g.get_pixel((x0 + x) as u32, (y0 + y) as u32).0[0] < 225;
        }
    }
    let (labels, n) = label_components(&mask, cw, ch, false);
    let stats = component_stats(&labels, n, cw);
    let area = (cw * ch) as f64;
    let bars = stats
        .iter()
        .skip(1)
        .filter(|s| {
            let a = s.area as f64;
            let fill = a / (s.width() * s.height()) as f64;
            a > 0.004 * area && fill > 0.85 && s.width() >= 4 && s.height() >= 4 && (s.bottom + 6 >= ch || s.left <= 6)
        })
        .count();
    bars >= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn draw_axes(g: &mut GrayImage, l: u32, t: u32, r: u32, b: u32, frame: bool) {
        for x in l..=r {
            g.put_pixel(x, b, Luma([0]));
            if frame {
                g.put_pixel(x, t, Luma([0]));
            }
        }
        for y in t..=b {
            g.put_pixel(l, y, Luma([0]));
            if frame {
                g.put_pixel(r, y, Luma([0]));
            }
        }
        // tick labels left of the y axis and below the x axis
        for k in 0..5 {
            let y = t + k * (b - t) / 4;
            for dx in 12..22 {
                for dy in 0..6 {
                    g.put_pixel(l - dx, (y + dy).min(b), Luma([0]));
                }
            }
            let x = l + k * (r - l) / 4;
            for dx in 0..6 {
                for dy in 8..16 {
                    g.put_pixel((x + dx).min(r), b + dy, Luma([0]));
                }
            }
        }
    }

    #[test]
    fn single_l_axes() {
        let mut g = GrayImage::from_pixel(600, 400, Luma([255]));
        draw_axes(&mut g, 80, 40, 560, 330, false);
        let r = detect_panels(&g);
        assert_eq!(r.panels.len(), 1, "{}", r.note);
        let p = &r.panels[0];
        assert!((p.inner.x0 - 80.0).abs() < 3.0 && (p.inner.y1 - 330.0).abs() < 3.0, "{:?}", p.inner);
        assert!(p.outer.x0 < 60.0 && p.outer.y1 > 340.0, "{:?}", p.outer);
    }

    #[test]
    fn chart_area_border_is_not_the_plot() {
        // Excel-style: grey border at the image edge around a framed plot with
        // horizontal grid lines (which form strip "frames" sharing its sides).
        let mut g = GrayImage::from_pixel(700, 480, Luma([255]));
        for x in 0..700 {
            g.put_pixel(x, 0, Luma([134]));
            g.put_pixel(x, 479, Luma([134]));
        }
        for y in 0..480 {
            g.put_pixel(0, y, Luma([134]));
            g.put_pixel(699, y, Luma([134]));
        }
        draw_axes(&mut g, 90, 15, 680, 420, true);
        for gy in [116, 218, 320] {
            for x in 90..=680 {
                g.put_pixel(x, gy, Luma([140]));
            }
        }
        let r = detect_panels(&g);
        assert_eq!(r.panels.len(), 1, "{}", r.note);
        let p = &r.panels[0];
        assert!((p.inner.x0 - 90.0).abs() < 4.0 && (p.inner.y0 - 15.0).abs() < 4.0, "{:?}", p.inner);
        assert!((p.inner.x1 - 680.0).abs() < 4.0 && (p.inner.y1 - 420.0).abs() < 4.0, "{:?}", p.inner);
    }

    #[test]
    fn two_by_one_frames() {
        let mut g = GrayImage::from_pixel(900, 400, Luma([255]));
        draw_axes(&mut g, 60, 40, 420, 330, true);
        draw_axes(&mut g, 520, 40, 880, 330, true);
        let r = detect_panels(&g);
        assert_eq!(r.panels.len(), 2, "{}", r.note);
        assert!(r.panels[0].inner.x0 < r.panels[1].inner.x0);
        assert!(r.panels[0].outer.x1 < r.panels[1].inner.x0);
    }
}
