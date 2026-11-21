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

/// Distance (Lab) from `p` to the segment `a`–`b`.
pub fn dist_to_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if l2 > 1e-9 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    lab_dist(p, [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]])
}

/// sRGB blends of `c` into `bg` (as a rasteriser anti-aliases), in Lab. Blends
/// curve through Lab, so a few samples keep the polyline within ~2 units.
pub fn fade_path(c: [u8; 3], bg: [u8; 3]) -> Vec<[f64; 3]> {
    (0..=4)
        .map(|k| {
            let a = 1.0 - k as f64 / 4.0;
            let mix = |i: usize| (a * c[i] as f64 + (1.0 - a) * bg[i] as f64).round() as u8;
            to_lab([mix(0), mix(1), mix(2)])
        })
        .collect()
}

/// Distance from `p` to a fade path: how far it is from being the colour or
/// one of its anti-aliased edge shades.
pub fn path_dist(p: [f64; 3], path: &[[f64; 3]]) -> f64 {
    path.windows(2).map(|s| dist_to_segment(p, s[0], s[1])).fold(f64::MAX, f64::min)
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
    filter_colors_among(img, settings, mask, region, &[])
}

/// The plot colours that compete with the foreground colour for a pixel:
/// the plot's other colours (grid grey, other series, text), without the
/// background, the foreground's own colour and its anti-aliased shades.
/// A colour and the fade paths of its rivals.
type Rivals = ([u8; 3], Vec<Vec<[f64; 3]>>);

fn rivals(fg: [u8; 3], bg: [u8; 3], palette: &[[u8; 3]]) -> Option<Rivals> {
    let (fl, bl) = (to_lab(fg), to_lab(bg));
    // The plot colour the foreground stands for (a picked or typed colour is
    // rarely exact). Without one, only the WebPlotDigitizer test applies.
    let own = palette
        .iter()
        .filter(|c| lab_dist(to_lab(**c), bl) > 10.0)
        .map(|c| (lab_dist(to_lab(*c), fl), *c))
        .filter(|(d, _)| *d < 30.0)
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())?
        .1;
    let own_path = fade_path(own, bg);
    let others = palette
        .iter()
        .map(|c| (to_lab(*c), *c))
        .filter(|(l, _)| lab_dist(*l, bl) > 10.0 && lab_dist(*l, to_lab(own)) > 25.0 && path_dist(*l, &own_path) > 10.0)
        .map(|(_, c)| fade_path(c, bg))
        .collect();
    Some((own, others))
}

/// [`filter_colors`], and in foreground mode a pixel must also not be a
/// better match for another of the plot's colours (`palette`, e.g. its
/// dominant colours) than for the foreground. Colours are compared with their
/// anti-aliased shades, so grid lines, the soft edges of neighbouring curves
/// and blends where curves cross stay out even within the colour distance.
pub fn filter_colors_among(
    img: &RgbaImage,
    settings: &AutoSettings,
    mask: Option<&[bool]>,
    region: Option<RectF>,
    palette: &[[u8; 3]],
) -> Vec<bool> {
    let rival = match settings.color_mode {
        ColorMode::Foreground => rivals(settings.fg, settings.bg, palette),
        ColorMode::Background => None,
    };
    let own_path = rival.as_ref().map(|(own, _)| fade_path(*own, settings.bg));
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
            if let (true, Some((_, others)), Some(own)) = (out[i], &rival, &own_path) {
                let l = to_lab(c);
                let d_own = path_dist(l, own);
                // A small margin keeps ties (shades near the background) with the foreground.
                if others.iter().any(|p| path_dist(l, p) + 3.0 < d_own) {
                    out[i] = false;
                }
            }
        }
    }
    out
}

/// The series colour under a painted stroke: the most common ink colour
/// (merged shades) among the `painted` pixels, preferring colours over grid
/// greys. `None` when the stroke covers no ink.
pub fn stroke_color(img: &RgbaImage, painted: &[bool], bg: [u8; 3]) -> Option<[u8; 3]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let bl = to_lab(bg);
    let mut tmp = RgbaImage::from_pixel(w as u32, h as u32, image::Rgba([bg[0], bg[1], bg[2], 255]));
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    let mut n = 0usize;
    let raw = img.as_raw();
    for (i, _) in painted.iter().enumerate().filter(|(_, p)| **p) {
        let c = [raw[4 * i], raw[4 * i + 1], raw[4 * i + 2]];
        if lab_dist(to_lab(c), bl) <= 22.0 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        tmp.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        n += 1;
    }
    if n == 0 || x1 > w || y1 > h {
        return None;
    }
    let r = RectF::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64);
    let cols: Vec<ColorCount> =
        dominant_colors(&tmp, Some(r), 8, 14.0).into_iter().filter(|c| lab_dist(to_lab(c.color), bl) > 22.0).collect();
    // Grid lines: light greys. Take a colour if one covers a fair share.
    let is_grid = |c: [u8; 3]| chroma(c) < 14.0 && to_lab(c)[0] > 45.0;
    let best = cols.iter().find(|c| !is_grid(c.color) && c.count as f64 >= 0.15 * n as f64).or(cols.first())?;
    Some(best.color)
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

    /// White image with a 3 px horizontal band of `c` at `y`.
    fn band(img: &mut RgbaImage, y: u32, c: [u8; 3]) {
        for yy in y..y + 3 {
            for x in 0..img.width() {
                img.put_pixel(x, yy, Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }

    #[test]
    fn rival_colours_stay_out() {
        // Within 120 of a teal curve: a grey grid line and the half-faded edge
        // of a brown curve. Both are better matches for their own colours.
        let mut img = RgbaImage::from_pixel(60, 40, Rgba([255, 255, 255, 255]));
        band(&mut img, 5, [75, 172, 198]);
        band(&mut img, 15, [190, 190, 190]);
        band(&mut img, 25, [150, 54, 52]);
        band(&mut img, 30, [202, 154, 153]);
        let s = AutoSettings { fg: [75, 172, 198], distance: 120.0, ..Default::default() };
        let rows = |bits: &[bool]| (0..40).filter(|y| bits[y * 60 + 30]).collect::<Vec<usize>>();
        assert_eq!(rows(&filter_colors(&img, &s, None, None)), [5, 6, 7, 15, 16, 17]);
        let palette = [[255, 255, 255], [75, 172, 198], [190, 190, 190], [150, 54, 52]];
        assert_eq!(rows(&filter_colors_among(&img, &s, None, None, &palette)), [5, 6, 7]);
        // An approximate pick still selects its curve.
        let s = AutoSettings { fg: [90, 160, 200], ..s };
        assert_eq!(rows(&filter_colors_among(&img, &s, None, None, &palette)), [5, 6, 7]);
        // Orange: the faded brown edge is within 120 but is brown's.
        let mut img2 = img.clone();
        band(&mut img2, 5, [247, 150, 70]);
        let s = AutoSettings { fg: [247, 150, 70], distance: 120.0, ..Default::default() };
        let palette = [[255, 255, 255], [247, 150, 70], [190, 190, 190], [150, 54, 52]];
        assert!(rows(&filter_colors(&img2, &s, None, None)).contains(&30));
        assert_eq!(rows(&filter_colors_among(&img2, &s, None, None, &palette)), [5, 6, 7]);
    }

    #[test]
    fn stroke_picks_the_curve_not_the_grid() {
        let mut img = RgbaImage::from_pixel(60, 40, Rgba([255, 255, 255, 255]));
        band(&mut img, 10, [60, 150, 175]);
        for y in 0..40 {
            for x in [20, 21, 40, 41] {
                img.put_pixel(x, y, Rgba([160, 160, 160, 255]));
            }
        }
        let painted: Vec<bool> = (0..60 * 40).map(|i| (5..20).contains(&(i / 60))).collect();
        let c = stroke_color(&img, &painted, [255, 255, 255]).unwrap();
        assert!(rgb_dist(c, [60, 150, 175]) < 10.0, "{c:?}");
        let empty: Vec<bool> = (0..60 * 40).map(|i| i / 60 >= 30).collect();
        assert_eq!(stroke_color(&img, &empty, [255, 255, 255]).map(|c| rgb_dist(c, [160, 160, 160]) < 10.0), Some(true));
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
