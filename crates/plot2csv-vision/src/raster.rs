//! Basic raster helpers shared by the vision and OCR crates.
//!
//! The resize functions reproduce OpenCV's `INTER_AREA` (downscale) and
//! `INTER_LINEAR` (upscale) semantics closely, because the indra OCR models
//! were trained on inputs produced by OpenCV.

use image::{GrayImage, Luma, RgbaImage};

/// Single-channel f32 raster, row-major.
#[derive(Clone, Debug)]
pub struct GrayF32 {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl GrayF32 {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: f32) {
        self.data[y * self.width + x] = v;
    }
}

/// ITU-R BT.601 luma, the same weights OpenCV uses for `COLOR_RGB2GRAY`.
pub fn rgba_to_gray(img: &RgbaImage) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for (o, p) in out.pixels_mut().zip(img.pixels()) {
        let [r, g, b, a] = p.0;
        // Composite onto white so transparent PNGs read as paper.
        let a = a as f32 / 255.0;
        let r = r as f32 * a + 255.0 * (1.0 - a);
        let g = g as f32 * a + 255.0 * (1.0 - a);
        let b = b as f32 * a + 255.0 * (1.0 - a);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        *o = Luma([y.round().clamp(0.0, 255.0) as u8]);
    }
    out
}

fn gray_to_f32(img: &GrayImage) -> GrayF32 {
    GrayF32 {
        width: img.width() as usize,
        height: img.height() as usize,
        data: img.as_raw().iter().map(|&v| v as f32).collect(),
    }
}

fn f32_to_gray(img: &GrayF32) -> GrayImage {
    let data = img.data.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    GrayImage::from_raw(img.width as u32, img.height as u32, data).expect("size")
}

/// Per-output-pixel list of (source index, weight) for an area resample along one axis.
fn area_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let s0 = d as f64 * scale;
            let s1 = ((d + 1) as f64 * scale).min(src as f64);
            let mut w = Vec::new();
            let mut s = s0.floor() as usize;
            while (s as f64) < s1 && s < src {
                let lo = (s as f64).max(s0);
                let hi = ((s + 1) as f64).min(s1);
                if hi > lo {
                    w.push((s, ((hi - lo) / (s1 - s0)) as f32));
                }
                s += 1;
            }
            if w.is_empty() {
                w.push((s0.floor().min((src - 1) as f64) as usize, 1.0));
            }
            w
        })
        .collect()
}

/// Per-output-pixel (i0, i1, frac) for a linear resample along one axis, with
/// OpenCV's half-pixel-centre convention.
fn linear_weights(src: usize, dst: usize) -> Vec<(usize, usize, f32)> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let s = ((d as f64 + 0.5) * scale - 0.5).max(0.0);
            let i0 = (s.floor() as usize).min(src - 1);
            let i1 = (i0 + 1).min(src - 1);
            (i0, i1, (s - i0 as f64) as f32)
        })
        .collect()
}

fn resize_axis_x(img: &GrayF32, dst_w: usize, area: bool) -> GrayF32 {
    let mut out = GrayF32::new(dst_w, img.height);
    if area {
        let wt = area_weights(img.width, dst_w);
        for y in 0..img.height {
            let row = &img.data[y * img.width..(y + 1) * img.width];
            for (x, ws) in wt.iter().enumerate() {
                out.data[y * dst_w + x] = ws.iter().map(|&(i, w)| row[i] * w).sum();
            }
        }
    } else {
        let wt = linear_weights(img.width, dst_w);
        for y in 0..img.height {
            let row = &img.data[y * img.width..(y + 1) * img.width];
            for (x, &(i0, i1, f)) in wt.iter().enumerate() {
                out.data[y * dst_w + x] = row[i0] * (1.0 - f) + row[i1] * f;
            }
        }
    }
    out
}

fn resize_axis_y(img: &GrayF32, dst_h: usize, area: bool) -> GrayF32 {
    let w = img.width;
    let mut out = GrayF32::new(w, dst_h);
    if area {
        let wt = area_weights(img.height, dst_h);
        for (y, ws) in wt.iter().enumerate() {
            let o = &mut out.data[y * w..(y + 1) * w];
            for &(i, wgt) in ws {
                let r = &img.data[i * w..(i + 1) * w];
                for x in 0..w {
                    o[x] += r[x] * wgt;
                }
            }
        }
    } else {
        let wt = linear_weights(img.height, dst_h);
        for (y, &(i0, i1, f)) in wt.iter().enumerate() {
            let o = &mut out.data[y * w..(y + 1) * w];
            let r0 = &img.data[i0 * w..(i0 + 1) * w];
            let r1 = &img.data[i1 * w..(i1 + 1) * w];
            for x in 0..w {
                o[x] = r0[x] * (1.0 - f) + r1[x] * f;
            }
        }
    }
    out
}

/// Resize like `cv2.resize(..., interpolation=INTER_AREA if area else INTER_LINEAR)`.
pub fn resize_gray(img: &GrayImage, dst_w: u32, dst_h: u32, area: bool) -> GrayImage {
    if img.width() == dst_w && img.height() == dst_h {
        return img.clone();
    }
    let f = gray_to_f32(img);
    // OpenCV's INTER_AREA falls back to linear when upscaling along an axis.
    let ax = area && (dst_w as usize) < f.width;
    let ay = area && (dst_h as usize) < f.height;
    let f = resize_axis_x(&f, dst_w as usize, ax);
    let f = resize_axis_y(&f, dst_h as usize, ay);
    f32_to_gray(&f)
}

/// Resize so the longer side is at most `max_side`; returns the image and the scale used.
pub fn downscale_to(img: &GrayImage, max_side: u32) -> (GrayImage, f32) {
    let long = img.width().max(img.height());
    if long <= max_side {
        return (img.clone(), 1.0);
    }
    let s = max_side as f32 / long as f32;
    let w = ((img.width() as f32 * s).round() as u32).max(1);
    let h = ((img.height() as f32 * s).round() as u32).max(1);
    (resize_gray(img, w, h, true), s)
}

/// Crop a region (may extend outside the image; outside is filled with `fill`).
pub fn crop_gray(img: &GrayImage, x0: i64, y0: i64, w: u32, h: u32, fill: u8) -> GrayImage {
    let mut out = GrayImage::from_pixel(w.max(1), h.max(1), Luma([fill]));
    let (iw, ih) = (img.width() as i64, img.height() as i64);
    let sx0 = x0.max(0);
    let sy0 = y0.max(0);
    let sx1 = (x0 + w as i64).min(iw);
    let sy1 = (y0 + h as i64).min(ih);
    if sx1 <= sx0 || sy1 <= sy0 {
        return out;
    }
    let src = img.as_raw();
    let dst_w = out.width() as usize;
    let dst = &mut *out;
    for y in sy0..sy1 {
        let s = (y * iw + sx0) as usize;
        let d = ((y - y0) as usize) * dst_w + (sx0 - x0) as usize;
        let n = (sx1 - sx0) as usize;
        dst[d..d + n].copy_from_slice(&src[s..s + n]);
    }
    out
}

/// 4- or 8-connected component labelling. Returns (labels, count) where labels
/// are 0 for background and 1..=count for components.
pub fn label_components(mask: &[bool], width: usize, height: usize, eight: bool) -> (Vec<u32>, u32) {
    let mut labels = vec![0u32; width * height];
    let mut next = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        next += 1;
        labels[start] = next;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let x = (i % width) as i64;
            let y = (i / width) as i64;
            let mut visit = |nx: i64, ny: i64| {
                if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                    return;
                }
                let j = ny as usize * width + nx as usize;
                if mask[j] && labels[j] == 0 {
                    labels[j] = next;
                    stack.push(j);
                }
            };
            visit(x - 1, y);
            visit(x + 1, y);
            visit(x, y - 1);
            visit(x, y + 1);
            if eight {
                visit(x - 1, y - 1);
                visit(x + 1, y - 1);
                visit(x - 1, y + 1);
                visit(x + 1, y + 1);
            }
        }
    }
    (labels, next)
}

/// Bounding box and pixel count of each labelled component (index 0 unused).
#[derive(Clone, Copy, Debug, Default)]
pub struct ComponentStats {
    pub left: usize,
    pub top: usize,
    pub right: usize,  // inclusive
    pub bottom: usize, // inclusive
    pub area: usize,
    pub sum_x: f64,
    pub sum_y: f64,
}

impl ComponentStats {
    pub fn width(&self) -> usize {
        self.right + 1 - self.left
    }
    pub fn height(&self) -> usize {
        self.bottom + 1 - self.top
    }
    pub fn centroid(&self) -> (f64, f64) {
        (self.sum_x / self.area as f64, self.sum_y / self.area as f64)
    }
}

pub fn component_stats(labels: &[u32], count: u32, width: usize) -> Vec<ComponentStats> {
    let mut st = vec![
        ComponentStats { left: usize::MAX, top: usize::MAX, ..Default::default() };
        count as usize + 1
    ];
    for (i, &l) in labels.iter().enumerate() {
        if l == 0 {
            continue;
        }
        let (x, y) = (i % width, i / width);
        let s = &mut st[l as usize];
        s.left = s.left.min(x);
        s.top = s.top.min(y);
        s.right = s.right.max(x);
        s.bottom = s.bottom.max(y);
        s.area += 1;
        s.sum_x += x as f64;
        s.sum_y += y as f64;
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_downscale_by_two_averages_blocks() {
        let img = GrayImage::from_raw(4, 2, vec![0, 100, 200, 200, 0, 100, 0, 0]).unwrap();
        let r = resize_gray(&img, 2, 1, true);
        assert_eq!(r.as_raw(), &vec![50, 100]);
    }

    #[test]
    fn linear_upscale_keeps_constant() {
        let img = GrayImage::from_pixel(3, 3, Luma([77]));
        let r = resize_gray(&img, 7, 5, false);
        assert!(r.pixels().all(|p| p.0[0] == 77));
    }

    #[test]
    fn crop_outside_fills() {
        let img = GrayImage::from_pixel(2, 2, Luma([10]));
        let c = crop_gray(&img, -1, -1, 3, 3, 255);
        assert_eq!(c.get_pixel(0, 0).0[0], 255);
        assert_eq!(c.get_pixel(1, 1).0[0], 10);
        assert_eq!(c.get_pixel(2, 2).0[0], 10);
    }

    #[test]
    fn components_four_vs_eight() {
        // diagonal pixels: 2 comps with 4-conn, 1 with 8-conn
        let m = vec![true, false, false, true];
        assert_eq!(label_components(&m, 2, 2, false).1, 2);
        assert_eq!(label_components(&m, 2, 2, true).1, 1);
    }
}
