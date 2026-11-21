//! Panel-level OCR plumbing shared by the app and the headless evaluator:
//! which crops to read, mapping results back, and merging into text items.

use image::GrayImage;
use plot2csv_core::geom::{Pt, RectF};
use plot2csv_core::project::{Panel, TextItem};

use crate::indra::TextLine;
use crate::labels::items_from_lines;

/// One crop to OCR.
#[derive(Clone)]
pub struct Crop {
    /// Top-left of the crop in corrected-image px.
    pub offset: Pt,
    pub img: GrayImage,
    /// Height of the original strip when the crop was rotated 90° clockwise.
    pub rotated_from_h: Option<f64>,
}

pub fn crop(g: &GrayImage, r: &RectF) -> (Pt, GrayImage) {
    let x0 = r.x0.floor().max(0.0);
    let y0 = r.y0.floor().max(0.0);
    let w = (r.x1.ceil().min(g.width() as f64) - x0).max(1.0) as u32;
    let h = (r.y1.ceil().min(g.height() as f64) - y0).max(1.0) as u32;
    (Pt::new(x0, y0), plot2csv_vision::raster::crop_gray(g, x0 as i64, y0 as i64, w, h, 255))
}

/// The crops read for a panel: its outer box, plus the strip left of the data
/// area rotated clockwise (vertical Y-axis titles).
pub fn panel_crops(g: &GrayImage, panel: &Panel) -> Vec<Crop> {
    let mut v = Vec::new();
    let (off, img) = crop(g, &panel.outer);
    v.push(Crop { offset: off, img, rotated_from_h: None });
    let strip = RectF::new(panel.outer.x0, panel.inner.y0, panel.inner.x0, panel.inner.y1);
    if strip.width() > 12.0 && strip.height() > 30.0 {
        let (off, img) = crop(g, &strip);
        let h = img.height() as f64;
        v.push(Crop { offset: off, img: image::imageops::rotate90(&img), rotated_from_h: Some(h) });
    }
    v
}

/// Map OCR lines of a crop back to corrected-image px. Returns (line, vertical).
pub fn map_lines(lines: Vec<TextLine>, c: &Crop) -> Vec<(TextLine, bool)> {
    lines
        .into_iter()
        .map(|mut l| {
            let vertical = c.rotated_from_h.is_some();
            if let Some(sh) = c.rotated_from_h {
                let [rx0, ry0, rx1, ry1] = l.bbox;
                l.bbox = [ry0, sh as f32 - rx1, ry1, sh as f32 - rx0];
                l.chars.clear();
            }
            l.bbox[0] += c.offset.x as f32;
            l.bbox[2] += c.offset.x as f32;
            l.bbox[1] += c.offset.y as f32;
            l.bbox[3] += c.offset.y as f32;
            for ch in l.chars.iter_mut() {
                ch.1 += c.offset.x as f32;
            }
            (l, vertical)
        })
        .collect()
}

fn rect(l: &TextLine) -> RectF {
    RectF::new(l.bbox[0] as f64, l.bbox[1] as f64, l.bbox[2] as f64, l.bbox[3] as f64)
}

/// Merge the lines of all crops into classified text items. Vertical-pass lines
/// are kept only when they are real words not overlapping horizontal text.
pub fn merge_items(collected: Vec<(TextLine, bool)>, inner: &RectF) -> Vec<TextItem> {
    let horizontal: Vec<RectF> = collected.iter().filter(|(_, v)| !v).map(|(l, _)| rect(l)).collect();
    let lines: Vec<(TextLine, bool)> = collected
        .into_iter()
        .filter(|(l, v)| {
            if !v {
                return true;
            }
            let r = rect(l);
            let letters = l.text.chars().filter(|c| c.is_alphabetic()).count();
            let overlaps = horizontal.iter().any(|h| h.intersect(&r).map(|i| i.area() > 0.2 * r.area().min(h.area())).unwrap_or(false));
            letters >= 2 && l.conf >= 0.6 && !overlaps && r.height() > 1.5 * r.width()
        })
        .collect();
    items_from_lines(&lines, inner)
}

/// Boxes (corrected px) of tick labels that the line detector missed: for each
/// tick mark without a nearby X/Y tick label, the ink blob next to it.
/// Returns (box, is_x_axis).
pub fn missing_tick_label_boxes(g: &GrayImage, panel: &Panel, items: &[TextItem]) -> Vec<([f32; 4], bool)> {
    use plot2csv_core::project::TextRole;
    let inner = panel.inner;
    let (ay, ax, xt, yt) = plot2csv_vision::axes_infer::axis_ticks(g, &inner);
    let (w, h) = (g.width() as i64, g.height() as i64);
    let dark = |x: i64, y: i64| x >= 0 && y >= 0 && x < w && y < h && g.get_pixel(x as u32, y as u32).0[0] < 150;
    let mut out = Vec::new();
    // Typical label height from existing tick labels.
    let lh = {
        let mut hs: Vec<f64> = items.iter().filter(|i| matches!(i.role, TextRole::XTick | TextRole::YTick)).map(|i| i.bbox.height()).collect();
        hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        hs.get(hs.len() / 2).copied().unwrap_or(0.025 * inner.height()).clamp(6.0, 40.0)
    };
    // Connected ink inside a search window, grouped into a single text row near `target`.
    let blob = |x0: i64, y0: i64, x1: i64, y1: i64, target: (f64, f64), horizontal_axis: bool| -> Option<[f32; 4]> {
        let (bw, bh) = ((x1 - x0).max(1) as usize, (y1 - y0).max(1) as usize);
        let mut mask = vec![false; bw * bh];
        for yy in 0..bh {
            for xx in 0..bw {
                mask[yy * bw + xx] = dark(x0 + xx as i64, y0 + yy as i64);
            }
        }
        let (labels, n) = plot2csv_vision::raster::label_components(&mask, bw, bh, true);
        let st = plot2csv_vision::raster::component_stats(&labels, n, bw);
        let comps: Vec<(f64, f64, f64, f64)> = st
            .iter()
            .skip(1)
            .filter(|s| s.area > 0 && (s.height() as f64) < 2.5 * lh && (s.width() as f64) < 6.0 * lh)
            .map(|s| ((x0 + s.left as i64) as f64, (y0 + s.top as i64) as f64, (x0 + s.right as i64 + 1) as f64, (y0 + s.bottom as i64 + 1) as f64))
            .collect();
        // Seed: the glyph-sized component nearest the target.
        let seed = comps
            .iter()
            .filter(|c| (c.3 - c.1) >= 0.4 * lh)
            .min_by(|a, b| {
                let da = (((a.0 + a.2) / 2.0 - target.0).powi(2) + ((a.1 + a.3) / 2.0 - target.1).powi(2)).sqrt();
                let db = (((b.0 + b.2) / 2.0 - target.0).powi(2) + ((b.1 + b.3) / 2.0 - target.1).powi(2)).sqrt();
                da.partial_cmp(&db).unwrap()
            })?;
        let mut bx = *seed;
        loop {
            let mut grown = false;
            for c in &comps {
                let inside = c.0 >= bx.0 && c.2 <= bx.2 && c.1 >= bx.1 && c.3 <= bx.3;
                if inside {
                    continue;
                }
                let v_overlap = (c.3.min(bx.3) - c.1.max(bx.1)) > 0.3 * (c.3 - c.1).min(bx.3 - bx.1).max(1.0) || ((c.1 + c.3) / 2.0 > bx.1 && (c.1 + c.3) / 2.0 < bx.3);
                let gap = (c.0 - bx.2).max(bx.0 - c.2);
                if v_overlap && gap < 0.7 * lh {
                    bx = (bx.0.min(c.0), bx.1.min(c.1), bx.2.max(c.2), bx.3.max(c.3));
                    grown = true;
                }
            }
            if !grown {
                break;
            }
        }
        let _ = horizontal_axis;
        Some([bx.0 as f32, bx.1 as f32, bx.2 as f32, bx.3 as f32])
    };
    let has_label = |role: TextRole, p: f64| {
        items.iter().any(|i| {
            i.role == role && {
                let c = if role == TextRole::XTick { i.bbox.center().x } else { i.bbox.center().y };
                (c - p).abs() < 0.6 * (if role == TextRole::XTick { i.bbox.width() } else { i.bbox.height() }).max(lh)
            }
        })
    };
    let tick_len = (0.012 * inner.height().min(inner.width())).clamp(3.0, 12.0) as i64 + 3;
    for &tx in &xt {
        if has_label(TextRole::XTick, tx) {
            continue;
        }
        let y0 = ay as i64 + tick_len;
        let y1 = ay as i64 + tick_len + (3.0 * lh) as i64;
        let half = (3.0 * lh) as i64;
        if let Some(b) = blob(tx as i64 - half, y0, tx as i64 + half, y1, (tx, y0 as f64 + lh / 2.0), true) {
            let cx = (b[0] + b[2]) as f64 / 2.0;
            if (cx - tx).abs() < 0.8 * (b[2] - b[0]) as f64 + 3.0 && !out.iter().any(|(o, _): &([f32; 4], bool)| o == &b) {
                out.push((b, true));
            }
        }
    }
    for &ty in &yt {
        if has_label(TextRole::YTick, ty) {
            continue;
        }
        let x1 = ax as i64 - tick_len;
        let x0 = x1 - (8.0 * lh) as i64;
        let half = (1.2 * lh) as i64;
        if let Some(b) = blob(x0, ty as i64 - half, x1, ty as i64 + half, (x1 as f64 - lh / 2.0, ty), false) {
            let cy = (b[1] + b[3]) as f64 / 2.0;
            if (cy - ty).abs() < 0.8 * (b[3] - b[1]) as f64 + 2.0 && !out.iter().any(|(o, _): &([f32; 4], bool)| o == &b) {
                out.push((b, false));
            }
        }
    }
    out
}

/// Turn recognised tick-label lines into tick items.
pub fn tick_items(lines: &[(TextLine, bool)]) -> Vec<TextItem> {
    use plot2csv_core::project::TextRole;
    lines
        .iter()
        .filter_map(|(l, is_x)| {
            let text = crate::labels::clean_label(&l.text);
            let value = crate::labels::label_value(&text)?;
            Some(TextItem {
                bbox: rect(l),
                text,
                conf: l.conf,
                role: if *is_x { TextRole::XTick } else { TextRole::YTick },
                value: Some(value),
                manual: false,
                vertical: false,
            })
        })
        .collect()
}
