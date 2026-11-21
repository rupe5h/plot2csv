//! Tick labels set as powers: 10², 10⁻¹, 2×10³. The line OCR drops the small
//! raised exponent ("10") or reads it as an ordinary digit ("101" for 10¹), so
//! the exponent is located from the glyph geometry and read on its own.

use image::{GrayImage, Luma};
use plot2csv_core::geom::RectF;
use plot2csv_core::project::TextItem;
use plot2csv_core::value::{normalize_number_text, parse_number};
use plot2csv_vision::raster::{component_stats, label_components};

use crate::indra::TextLine;
use crate::labels::classify;

/// Grey level below which a pixel is ink.
const INK: u8 = 150;

/// A label whose exponent glyphs were located (corrected-image px).
#[derive(Clone, Debug)]
pub struct PowerLabel {
    /// Index of the text item.
    pub item: usize,
    pub base: RectF,
    pub exp: RectF,
}

/// The glyphs of every found label pasted apart on a white canvas, so each
/// recognition crop sees nothing but its own glyphs (no tick marks, no
/// neighbouring digits). `boxes` are in canvas px: base, exponent, base, …
pub struct PowerScan {
    pub labels: Vec<PowerLabel>,
    pub canvas: GrayImage,
    pub boxes: Vec<[f32; 4]>,
}

/// Ink component in page px (`x1`, `y1` exclusive).
#[derive(Clone, Copy, Debug)]
struct Comp {
    id: u32,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Comp {
    fn w(&self) -> f64 {
        self.x1 - self.x0
    }
    fn h(&self) -> f64 {
        self.y1 - self.y0
    }
    fn cx(&self) -> f64 {
        (self.x0 + self.x1) / 2.0
    }
    fn cy(&self) -> f64 {
        (self.y0 + self.y1) / 2.0
    }
}

/// Connected ink inside a window of the page.
struct Window {
    x0: i64,
    y0: i64,
    w: usize,
    h: usize,
    labels: Vec<u32>,
    comps: Vec<Comp>,
}

impl Window {
    fn new(g: &GrayImage, r: &RectF) -> Option<Self> {
        let x0 = r.x0.floor().max(0.0) as i64;
        let y0 = r.y0.floor().max(0.0) as i64;
        let x1 = (r.x1.ceil() as i64).min(g.width() as i64);
        let y1 = (r.y1.ceil() as i64).min(g.height() as i64);
        if x1 - x0 < 4 || y1 - y0 < 4 {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mask: Vec<bool> =
            (0..w * h).map(|i| g.get_pixel((x0 + (i % w) as i64) as u32, (y0 + (i / w) as i64) as u32).0[0] < INK).collect();
        let (labels, n) = label_components(&mask, w, h, true);
        let comps = component_stats(&labels, n, w)
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, s)| s.area > 0)
            .map(|(id, s)| Comp {
                id: id as u32,
                x0: (x0 + s.left as i64) as f64,
                y0: (y0 + s.top as i64) as f64,
                x1: (x0 + s.right as i64 + 1) as f64,
                y1: (y0 + s.bottom as i64 + 1) as f64,
            })
            .collect();
        Some(Self { x0, y0, w, h, labels, comps })
    }

    /// Pixels of the given components plus their anti-aliased fringe, relative
    /// to the group's top-left corner: (w, h, [(dx, dy, grey)]).
    fn group_pixels(&self, g: &GrayImage, comps: &[Comp]) -> (u32, u32, Vec<(u32, u32, u8)>) {
        let ids: Vec<u32> = comps.iter().map(|c| c.id).collect();
        let gx0 = comps.iter().map(|c| c.x0).fold(f64::MAX, f64::min) as i64 - 1;
        let gy0 = comps.iter().map(|c| c.y0).fold(f64::MAX, f64::min) as i64 - 1;
        let gx1 = comps.iter().map(|c| c.x1).fold(f64::MIN, f64::max) as i64 + 1;
        let gy1 = comps.iter().map(|c| c.y1).fold(f64::MIN, f64::max) as i64 + 1;
        let label_at = |x: i64, y: i64| -> Option<u32> {
            let (lx, ly) = (x - self.x0, y - self.y0);
            (lx >= 0 && ly >= 0 && (lx as usize) < self.w && (ly as usize) < self.h)
                .then(|| self.labels[ly as usize * self.w + lx as usize])
        };
        let mut px = Vec::new();
        for y in gy0..gy1 {
            for x in gx0..gx1 {
                let Some(l) = label_at(x, y) else { continue };
                let keep = if l != 0 {
                    ids.contains(&l)
                } else {
                    (-1..=1).any(|dy| (-1..=1).any(|dx| label_at(x + dx, y + dy).is_some_and(|n| n != 0 && ids.contains(&n))))
                };
                if keep {
                    px.push(((x - gx0) as u32, (y - gy0) as u32, g.get_pixel(x as u32, y as u32).0[0]));
                }
            }
        }
        ((gx1 - gx0) as u32, (gy1 - gy0) as u32, px)
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn union(cs: &[Comp]) -> RectF {
    RectF::new(
        cs.iter().map(|c| c.x0).fold(f64::MAX, f64::min),
        cs.iter().map(|c| c.y0).fold(f64::MAX, f64::min),
        cs.iter().map(|c| c.x1).fold(f64::MIN, f64::max),
        cs.iter().map(|c| c.y1).fold(f64::MIN, f64::max),
    )
}

/// Split the glyphs of one label into base and exponent. `label` is the OCR box,
/// which may or may not include the exponent.
fn split_power(win: &Window, label: &RectF) -> Option<(Vec<Comp>, Vec<Comp>)> {
    let lh = label.height().max(4.0);
    let (wx0, wy0) = (win.x0 as f64, win.y0 as f64);
    let (wx1, wy1) = (wx0 + win.w as f64, wy0 + win.h as f64);
    // Glyph-sized ink not cut by the window edge (axis lines, frames, other labels).
    let comps: Vec<Comp> = win
        .comps
        .iter()
        .copied()
        .filter(|c| c.h() <= 1.6 * lh && c.w() <= 2.0 * lh && c.x0 > wx0 && c.y0 > wy0 && c.x1 < wx1 && c.y1 < wy1)
        .collect();
    let in_label = |c: &Comp| c.cx() >= label.x0 - 0.2 * lh && c.cx() <= label.x1 + 0.2 * lh;
    let hmax = comps.iter().filter(|c| in_label(c)).map(|c| c.h()).fold(0.0, f64::max);
    if hmax < 5.0 {
        return None;
    }
    // The base sits on the baseline of the tallest glyphs.
    let tall: Vec<&Comp> = comps.iter().filter(|c| in_label(c) && c.h() >= 0.8 * hmax).collect();
    let baseline = median(tall.iter().map(|c| c.y1).collect());
    let top = median(tall.iter().map(|c| c.y0).collect());
    let hh = baseline - top;
    if hh < 5.0 {
        return None;
    }
    let mut main: Vec<Comp> =
        comps.iter().copied().filter(|c| in_label(c) && c.h() >= 0.5 * hh && (c.y1 - baseline).abs() <= 0.15 * hh + 1.0).collect();
    if main.len() < 2 {
        return None;
    }
    main.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap());
    let last = main.last().unwrap().x1;
    // Exponent: smaller glyphs raised above the baseline, from half-way up the
    // base to sitting right on top of it (a minus is a short dash), chained
    // right after the base.
    let sup_glyph = |c: &Comp| {
        c.h() >= 0.45 * hh
            && c.h() <= 0.9 * hh
            && c.y1 <= baseline - 0.25 * hh
            && c.y1 >= top - 0.1 * hh
            && c.y0 <= top + 0.15 * hh
            && c.y0 >= top - 0.8 * hh
    };
    let sup_dash = |c: &Comp| {
        c.h() <= (0.2 * hh).max(2.0) && c.w() >= c.h() && c.w() <= 0.9 * hh && c.cy() >= top - 0.5 * hh && c.cy() <= baseline - 0.45 * hh
    };
    let in_band = |c: &Comp| c.y1 > top - 0.7 * hh && c.y0 < baseline;
    let main_ids: Vec<u32> = main.iter().map(|c| c.id).collect();
    let mut right: Vec<Comp> = comps.iter().copied().filter(|c| c.x0 >= last - 1.0 && in_band(c) && !main_ids.contains(&c.id)).collect();
    right.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap());
    let mut exp: Vec<Comp> = Vec::new();
    let mut edge = last;
    for c in right {
        if c.x0 - edge > 0.6 * hh || !(sup_glyph(&c) || sup_dash(&c)) {
            break;
        }
        edge = edge.max(c.x1);
        exp.push(c);
    }
    // Trailing dashes are tick marks; an exponent needs at least one glyph.
    while exp.last().is_some_and(|c| !sup_glyph(c)) {
        exp.pop();
    }
    if exp.is_empty() {
        return None;
    }
    let exp_ids: Vec<u32> = exp.iter().map(|c| c.id).collect();
    // Base: the label's glyphs left of the exponent (sign, multiplier, ×, "10").
    let base: Vec<Comp> =
        comps.iter().copied().filter(|c| in_label(c) && in_band(c) && c.x1 <= last + 1.0 && !exp_ids.contains(&c.id)).collect();
    Some((base, exp))
}

/// Does the OCR text look like it could be a power of ten ("10", "101", "10°", "2x10")?
fn maybe_power(text: &str) -> bool {
    let t = normalize_number_text(text).replace(' ', "");
    t.chars().count() <= 10 && (t.starts_with("10") || t.starts_with("-10") || t.contains("x10") || t.contains("*10"))
}

/// Find tick labels (left of or below the data area) written as powers.
pub fn find_power_labels(g: &GrayImage, inner: &RectF, items: &[TextItem]) -> Option<PowerScan> {
    let mut found: Vec<(PowerLabel, [(u32, u32, Vec<(u32, u32, u8)>); 2])> = Vec::new();
    for (k, it) in items.iter().enumerate() {
        if it.manual || it.vertical || !maybe_power(&it.text) {
            continue;
        }
        let b = it.bbox;
        let lh = b.height().max(4.0);
        let c = b.center();
        let (below_amt, left_amt) = (c.y - inner.y1, inner.x0 - c.x);
        if below_amt <= 0.0 && left_amt <= 0.0 {
            continue;
        }
        // Room for an exponent on the right and above, but not past the axis line.
        let mut r = RectF::new(b.x0 - 0.3 * lh, b.y0 - 0.8 * lh, b.x1 + 2.0 * lh, b.y1 + 0.3 * lh);
        if below_amt >= left_amt {
            r.y0 = r.y0.max(inner.y1 + 1.0);
        } else {
            r.x1 = r.x1.min(inner.x0 - 1.0);
        }
        let Some(win) = Window::new(g, &r) else { continue };
        let Some((base, exp)) = split_power(&win, &b) else { continue };
        let pixels = [win.group_pixels(g, &base), win.group_pixels(g, &exp)];
        found.push((PowerLabel { item: k, base: union(&base), exp: union(&exp) }, pixels));
    }
    if found.is_empty() {
        return None;
    }
    // One row per label: base, gap, exponent; rows far enough apart that the
    // recogniser's vertical padding never reaches the next row.
    let max_h = found.iter().flat_map(|(_, p)| p.iter().map(|g| g.1)).max().unwrap_or(1);
    let (pad_y, gap_x) = (2 * max_h + 4, max_h + 4);
    let row_h = max_h + 2 * pad_y;
    let width = found.iter().map(|(_, p)| p[0].0 + p[1].0).max().unwrap_or(1) + 2 * gap_x + gap_x;
    let mut canvas = GrayImage::from_pixel(width, row_h * found.len() as u32, Luma([255]));
    let mut boxes = Vec::new();
    for (row, (_, groups)) in found.iter().enumerate() {
        let mut x = gap_x;
        for (gw, gh, px) in groups {
            let y = row as u32 * row_h + pad_y;
            for &(dx, dy, v) in px {
                canvas.put_pixel(x + dx, y + dy, Luma([v]));
            }
            // The group box has a 1 px fringe; the OCR box is the ink itself.
            boxes.push([(x + 1) as f32, (y + 1) as f32, (x + gw - 1) as f32, (y + gh - 1) as f32]);
            x += gw + gap_x;
        }
    }
    Some(PowerScan { labels: found.into_iter().map(|f| f.0).collect(), canvas, boxes })
}

/// "10", "-10", "2x10" from an OCR'd base, tolerating O/0 and l/1 confusions.
fn power_base(text: &str) -> Option<String> {
    let t: String = normalize_number_text(text)
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            'O' | 'o' => '0',
            'l' | 'I' | '|' => '1',
            '·' | '•' | 'X' | '*' => 'x',
            c => c,
        })
        .collect();
    let prefix = t.strip_suffix("10")?;
    let ok = match prefix {
        "" | "-" => true,
        p => p.strip_suffix('x').and_then(parse_number).is_some(),
    };
    ok.then_some(t)
}

/// An OCR'd exponent as a signed integer string ("2", "-1").
fn exponent(text: &str) -> Option<String> {
    let t: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '⁰' => '0',
            '¹' | 'l' | 'I' | '|' | 'i' | '!' => '1',
            '²' => '2',
            '³' => '3',
            '⁴' => '4',
            '⁵' => '5',
            '⁶' => '6',
            '⁷' => '7',
            '⁸' => '8',
            '⁹' => '9',
            '⁻' | '−' | '–' | '—' | '‐' | '_' | '~' => '-',
            '⁺' => '+',
            c => c,
        })
        .collect();
    let k: i32 = t.strip_prefix('+').unwrap_or(&t).parse().ok()?;
    let digits = t.trim_start_matches(['-', '+']);
    (!digits.is_empty() && digits.len() <= 2 && k.abs() <= 30).then(|| k.to_string())
}

/// Rewrite the labels of `scan` from the recognised base/exponent lines (in
/// `scan.boxes` order): text "10^2", value 100, box covering the exponent.
/// Returns the number of labels rewritten.
pub fn apply_power_labels(items: &mut [TextItem], scan: &PowerScan, lines: &[TextLine], inner: &RectF) -> usize {
    let mut n = 0;
    let mut reclassify = false;
    for (k, pl) in scan.labels.iter().enumerate() {
        let (Some(bl), Some(el)) = (lines.get(2 * k), lines.get(2 * k + 1)) else { continue };
        let Some(e) = exponent(&el.text) else { continue };
        let it = &mut items[pl.item];
        // Fall back to the first pass when the base alone was not readable.
        let Some(base) = power_base(&bl.text).or_else(|| normalize_number_text(&it.text).starts_with("10").then(|| "10".to_string()))
        else {
            continue;
        };
        let text = format!("{base}^{e}");
        let Some(v) = parse_number(&text) else { continue };
        reclassify |= it.value.is_none();
        it.text = text;
        it.value = Some(v);
        it.bbox = RectF::new(pl.base.x0.min(pl.exp.x0), pl.base.y0.min(pl.exp.y0), pl.base.x1.max(pl.exp.x1), pl.base.y1.max(pl.exp.y1));
        it.conf = it.conf.min(el.conf);
        n += 1;
    }
    // A label that was not numeric before may have been classified as a title.
    if reclassify {
        classify(items, inner);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use plot2csv_core::project::TextRole;

    fn rect(g: &mut GrayImage, x0: u32, y0: u32, x1: u32, y1: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                g.put_pixel(x, y, Luma([0]));
            }
        }
    }

    /// A "glyph": an outlined box (like a "0") so it is one component.
    fn glyph(g: &mut GrayImage, x0: u32, y0: u32, x1: u32, y1: u32) {
        rect(g, x0, y0, x1, y0 + 1);
        rect(g, x0, y1 - 1, x1, y1);
        rect(g, x0, y0, x0 + 1, y1);
        rect(g, x1 - 1, y0, x1, y1);
    }

    fn item(b: RectF, text: &str, role: TextRole) -> TextItem {
        TextItem { bbox: b, text: text.into(), conf: 0.9, role, value: crate::labels::label_value(text), manual: false, vertical: false }
    }

    fn line(text: &str) -> TextLine {
        TextLine { bbox: [0.0; 4], text: text.into(), conf: 0.95, chars: vec![] }
    }

    /// Y label "10⁻¹" left of the axis at x=90, with a tick mark next to it.
    fn y_label_image() -> GrayImage {
        let mut g = GrayImage::from_pixel(200, 120, Luma([255]));
        rect(&mut g, 90, 0, 91, 120); // axis
        rect(&mut g, 86, 60, 90, 61); // outward tick mark
        glyph(&mut g, 55, 57, 62, 69); // "1"
        glyph(&mut g, 64, 57, 72, 69); // "0"
        rect(&mut g, 74, 56, 78, 57); // superscript minus
        glyph(&mut g, 80, 53, 85, 61); // superscript "1"
        g
    }

    #[test]
    fn finds_raised_exponent_and_minus() {
        let g = y_label_image();
        let inner = RectF::new(90.0, 5.0, 190.0, 100.0);
        // OCR box covers only "10".
        let items = vec![item(RectF::new(53.0, 55.0, 79.0, 70.0), "10", TextRole::YTick)];
        let scan = find_power_labels(&g, &inner, &items).expect("exponent found");
        assert_eq!(scan.labels.len(), 1);
        let pl = &scan.labels[0];
        assert_eq!((pl.base.x0, pl.base.x1), (55.0, 72.0));
        assert_eq!((pl.exp.x0, pl.exp.y0, pl.exp.x1, pl.exp.y1), (74.0, 53.0, 85.0, 61.0), "tick mark must not join the exponent");
        assert_eq!(scan.boxes.len(), 2);
        // The canvas holds the exponent glyphs and nothing of the axis.
        let b = scan.boxes[1];
        let ink = |x0: f32, x1: f32, y0: f32, y1: f32| {
            (y0 as u32..y1 as u32)
                .flat_map(|y| (x0 as u32..x1 as u32).map(move |x| (x, y)))
                .filter(|&(x, y)| scan.canvas.get_pixel(x, y).0[0] < INK)
                .count()
        };
        assert!(ink(b[0], b[2], b[1], b[3]) > 10);
        assert_eq!(ink(b[2] + 1.0, scan.canvas.width() as f32, b[1] - 3.0, b[3] + 3.0), 0);

        let mut items = items;
        let n = apply_power_labels(&mut items, &scan, &[line("10"), line("-1")], &inner);
        assert_eq!(n, 1);
        assert_eq!(items[0].text, "10^-1");
        assert_eq!(items[0].value, Some(0.1));
        assert_eq!(items[0].bbox.x1, 85.0);
    }

    #[test]
    fn exponent_read_as_digit_is_split_off() {
        // "10¹" read as "101": the OCR box covers the exponent too.
        let mut g = GrayImage::from_pixel(200, 120, Luma([255]));
        rect(&mut g, 90, 0, 91, 120);
        glyph(&mut g, 61, 57, 68, 69);
        glyph(&mut g, 70, 57, 78, 69);
        glyph(&mut g, 80, 53, 85, 61);
        let inner = RectF::new(90.0, 5.0, 190.0, 100.0);
        let mut items = vec![item(RectF::new(59.0, 52.0, 86.0, 70.0), "101", TextRole::YTick)];
        let scan = find_power_labels(&g, &inner, &items).expect("exponent found");
        assert_eq!(scan.labels[0].exp.x0, 80.0);
        apply_power_labels(&mut items, &scan, &[line("10"), line("1")], &inner);
        assert_eq!(items[0].value, Some(10.0));
    }

    #[test]
    fn exponent_sitting_on_top_of_the_base() {
        // Exponent bottom level with the top of "10" (tests/fixtures/log_markers.png).
        let mut g = GrayImage::from_pixel(200, 120, Luma([255]));
        rect(&mut g, 90, 0, 91, 120);
        glyph(&mut g, 61, 57, 68, 69);
        glyph(&mut g, 70, 57, 78, 69);
        glyph(&mut g, 80, 50, 85, 58);
        let inner = RectF::new(90.0, 5.0, 190.0, 100.0);
        let mut items = vec![item(RectF::new(59.0, 48.0, 86.0, 70.0), "10°", TextRole::YTick)];
        let scan = find_power_labels(&g, &inner, &items).expect("exponent found");
        apply_power_labels(&mut items, &scan, &[line("10"), line("3")], &inner);
        assert_eq!(items[0].value, Some(1000.0));
    }

    #[test]
    fn plain_numbers_and_tick_marks_are_left_alone() {
        // "100" on the baseline, then an outward tick mark: no exponent.
        let mut g = GrayImage::from_pixel(200, 120, Luma([255]));
        rect(&mut g, 90, 0, 91, 120);
        rect(&mut g, 84, 62, 90, 63);
        glyph(&mut g, 55, 57, 62, 69);
        glyph(&mut g, 64, 57, 72, 69);
        glyph(&mut g, 74, 57, 82, 69);
        let inner = RectF::new(90.0, 5.0, 190.0, 100.0);
        let items = vec![item(RectF::new(53.0, 55.0, 83.0, 70.0), "100", TextRole::YTick)];
        assert!(find_power_labels(&g, &inner, &items).is_none());
    }

    #[test]
    fn x_label_below_axis() {
        // "10³" under the X axis at y=50, with a tick mark above its centre.
        let mut g = GrayImage::from_pixel(200, 120, Luma([255]));
        rect(&mut g, 0, 50, 200, 51);
        rect(&mut g, 72, 51, 73, 56);
        glyph(&mut g, 60, 62, 67, 74);
        glyph(&mut g, 69, 62, 77, 74);
        glyph(&mut g, 79, 58, 84, 66);
        let inner = RectF::new(10.0, 5.0, 190.0, 50.0);
        let mut items = vec![item(RectF::new(58.0, 60.0, 79.0, 75.0), "10", TextRole::XTick)];
        let scan = find_power_labels(&g, &inner, &items).expect("exponent found");
        apply_power_labels(&mut items, &scan, &[line("1O"), line("3")], &inner);
        assert_eq!(items[0].text, "10^3");
        assert_eq!(items[0].value, Some(1000.0));
    }

    #[test]
    fn base_and_exponent_text() {
        assert_eq!(power_base("10").as_deref(), Some("10"));
        assert_eq!(power_base("1O").as_deref(), Some("10"));
        assert_eq!(power_base("2×10").as_deref(), Some("2x10"));
        assert_eq!(power_base("-10").as_deref(), Some("-10"));
        assert_eq!(power_base("20"), None);
        assert_eq!(power_base("e10"), None);
        assert_eq!(exponent("-1").as_deref(), Some("-1"));
        assert_eq!(exponent("−2").as_deref(), Some("-2"));
        assert_eq!(exponent("l").as_deref(), Some("1"));
        assert_eq!(exponent("²").as_deref(), Some("2"));
        assert_eq!(exponent("+3").as_deref(), Some("3"));
        assert_eq!(exponent("°"), None);
        assert_eq!(exponent("-"), None);
        assert_eq!(exponent("123"), None);
    }
}
