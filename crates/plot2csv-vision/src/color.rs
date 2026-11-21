//! Colour utilities: distances, Lab conversion, dominant colours and colour filtering.

use image::RgbaImage;
use plot2csv_core::dataset::{AutoSettings, ColorMode};
use plot2csv_core::geom::RectF;

#[inline]
pub fn rgb_dist(a: [u8; 3], b: [u8; 3]) -> f64 {
    let d0 = a[0] as f64 - b[0] as f64;
    let d1 = a[1] as f64 - b[1] as f64;
    let d2 = a[2] as f64 - b[2] as f64;
    (d0 * d0 + d1 * d1 + d2 * d2).sqrt()
}

#[inline]
pub fn rgb(p: &image::Rgba<u8>) -> [u8; 3] {
    [p.0[0], p.0[1], p.0[2]]
}

fn srgb_to_linear(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// CIE L*a*b* (D65).
pub fn to_lab(c: [u8; 3]) -> [f64; 3] {
    let (r, g, b) = (srgb_to_linear(c[0] as f64), srgb_to_linear(c[1] as f64), srgb_to_linear(c[2] as f64));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn lab_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Chroma (saturation in Lab).
pub fn chroma(c: [u8; 3]) -> f64 {
    let l = to_lab(c);
    (l[1] * l[1] + l[2] * l[2]).sqrt()
}

/// A colour and its pixel count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorCount {
    pub color: [u8; 3],
    pub count: usize,
}

/// Dominant colours inside `region` (or the whole image), most frequent first.
/// Similar colours (Lab distance < `merge`) are merged.
pub fn dominant_colors(img: &RgbaImage, region: Option<RectF>, max: usize, merge: f64) -> Vec<ColorCount> {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let r = region.unwrap_or(RectF::new(0.0, 0.0, w, h)).clamp_to(w, h);
    // 5-bit-per-channel histogram with running colour sums.
    let mut hist: std::collections::HashMap<u16, (usize, [u64; 3])> = std::collections::HashMap::new();
    let step = (((r.area()) / 400_000.0).sqrt().floor() as u32).max(1);
    let mut y = r.y0 as u32;
    while (y as f64) < r.y1 {
        let mut x = r.x0 as u32;
        while (x as f64) < r.x1 {
            let p = img.get_pixel(x, y).0;
            let key = ((p[0] as u16 >> 3) << 10) | ((p[1] as u16 >> 3) << 5) | (p[2] as u16 >> 3);
            let e = hist.entry(key).or_insert((0, [0; 3]));
            e.0 += 1;
            e.1[0] += p[0] as u64;
            e.1[1] += p[1] as u64;
            e.1[2] += p[2] as u64;
            x += step;
        }
        y += step;
    }
    let mut bins: Vec<ColorCount> = hist
        .into_values()
        .map(|(n, s)| ColorCount { color: [(s[0] / n as u64) as u8, (s[1] / n as u64) as u8, (s[2] / n as u64) as u8], count: n })
        .collect();
    bins.sort_by(|a, b| b.count.cmp(&a.count));
    let mut out: Vec<(ColorCount, [f64; 3])> = Vec::new();
    for b in bins {
        let lab = to_lab(b.color);
        if let Some(o) = out.iter_mut().find(|o| lab_dist(o.1, lab) < merge) {
            o.0.count += b.count;
        } else {
            out.push((b, lab));
        }
    }
    out.sort_by(|a, b| b.0.count.cmp(&a.0.count));
    out.into_iter().take(max).map(|o| o.0).collect()
}

/// Which pixels pass the colour test (and the mask, if any), within `region`.
/// Returns a full-image bitmap.
pub fn filter_colors(img: &RgbaImage, settings: &AutoSettings, mask: Option<&[bool]>, region: Option<RectF>) -> Vec<bool> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = vec![false; w * h];
    let r = region.unwrap_or(RectF::new(0.0, 0.0, w as f64, h as f64)).clamp_to(w as f64, h as f64);
    let (x0, y0, x1, y1) = (r.x0.floor() as usize, r.y0.floor() as usize, r.x1.ceil() as usize, r.y1.ceil() as usize);
    let raw = img.as_raw();
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let i = y * w + x;
            if let Some(m) = mask {
                if !m[i] {
                    continue;
                }
            }
            let c = [raw[4 * i], raw[4 * i + 1], raw[4 * i + 2]];
            out[i] = match settings.color_mode {
                ColorMode::Foreground => rgb_dist(c, settings.fg) <= settings.distance,
                ColorMode::Background => rgb_dist(c, settings.bg) > settings.distance,
            };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn dominant_finds_two() {
        let img = RgbaImage::from_fn(100, 100, |x, _| if x < 70 { Rgba([255, 255, 255, 255]) } else { Rgba([255, 165, 0, 255]) });
        let d = dominant_colors(&img, None, 5, 10.0);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].color, [255, 255, 255]);
        assert!(rgb_dist(d[1].color, [255, 165, 0]) < 10.0);
    }

    #[test]
    fn filter_fg_and_bg() {
        let img = RgbaImage::from_fn(4, 1, |x, _| if x == 2 { Rgba([255, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        let mut s = AutoSettings { fg: [250, 10, 10], distance: 30.0, ..Default::default() };
        assert_eq!(filter_colors(&img, &s, None, None), vec![false, false, true, false]);
        s.color_mode = ColorMode::Background;
        assert_eq!(filter_colors(&img, &s, None, None), vec![false, false, true, false]);
    }
}
