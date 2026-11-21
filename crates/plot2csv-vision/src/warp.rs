//! Perspective warp and simple photo enhancement.

use image::{Rgba, RgbaImage};
use plot2csv_core::geom::{Homography, Pt};

/// Bilinear sample with white outside the image.
#[inline]
pub fn sample_bilinear(img: &RgbaImage, x: f64, y: f64) -> [u8; 4] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let x = x - 0.5;
    let y = y - 0.5;
    let x0 = x.floor() as i64;
    let y0 = y.floor() as i64;
    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;
    let get = |xx: i64, yy: i64| -> [f32; 4] {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            [255.0, 255.0, 255.0, 255.0]
        } else {
            let p = img.get_pixel(xx as u32, yy as u32).0;
            [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
        }
    };
    let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = a[i] * (1.0 - fx) + b[i] * fx;
        let bot = c[i] * (1.0 - fx) + d[i] * fx;
        out[i] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Produce the `out_w × out_h` corrected image given `h` mapping source -> output.
pub fn warp_perspective(src: &RgbaImage, h: &Homography, out_w: u32, out_h: u32) -> RgbaImage {
    let inv = h.inverse().unwrap_or_default();
    let mut out = RgbaImage::new(out_w.max(1), out_h.max(1));
    for (x, y, p) in out.enumerate_pixels_mut() {
        let s = inv.apply(Pt::new(x as f64 + 0.5, y as f64 + 0.5));
        *p = Rgba(sample_bilinear(src, s.x, s.y));
    }
    out
}

/// Flatten uneven illumination and stretch contrast: divide each channel by a
/// heavily blurred estimate of the paper colour (from a downscaled max filter).
pub fn enhance_photo(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width(), src.height());
    if w < 8 || h < 8 {
        return src.clone();
    }
    // Background estimate on a coarse grid: per cell, the 90th percentile of luma.
    let cell = ((w.max(h) / 32).max(8)) as usize;
    let gw = (w as usize).div_ceil(cell);
    let gh = (h as usize).div_ceil(cell);
    let mut bg = vec![[255f32; 3]; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut px: Vec<(u32, [u8; 3])> = Vec::new();
            for y in (gy * cell)..((gy + 1) * cell).min(h as usize) {
                for x in (gx * cell)..((gx + 1) * cell).min(w as usize) {
                    let p = src.get_pixel(x as u32, y as u32).0;
                    let l = p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114;
                    px.push((l, [p[0], p[1], p[2]]));
                }
            }
            if px.is_empty() {
                continue;
            }
            px.sort_unstable_by_key(|p| p.0);
            let p = px[(px.len() * 9 / 10).min(px.len() - 1)].1;
            bg[gy * gw + gx] = [p[0] as f32, p[1] as f32, p[2] as f32];
        }
    }
    // Smooth the grid (3x3 box, twice) to avoid seams.
    for _ in 0..2 {
        let old = bg.clone();
        for gy in 0..gh {
            for gx in 0..gw {
                let mut s = [0f32; 3];
                let mut n = 0.0;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (x, y) = (gx as i64 + dx, gy as i64 + dy);
                        if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
                            let v = old[y as usize * gw + x as usize];
                            for i in 0..3 {
                                s[i] += v[i];
                            }
                            n += 1.0;
                        }
                    }
                }
                bg[gy * gw + gx] = [s[0] / n, s[1] / n, s[2] / n];
            }
        }
    }
    let mut out = src.clone();
    for (x, y, p) in out.enumerate_pixels_mut() {
        // bilinear lookup in the grid
        let fx = (x as f32 + 0.5) / cell as f32 - 0.5;
        let fy = (y as f32 + 0.5) / cell as f32 - 0.5;
        let x0 = fx.floor().clamp(0.0, (gw - 1) as f32) as usize;
        let y0 = fy.floor().clamp(0.0, (gh - 1) as f32) as usize;
        let x1 = (x0 + 1).min(gw - 1);
        let y1 = (y0 + 1).min(gh - 1);
        let tx = (fx - x0 as f32).clamp(0.0, 1.0);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for i in 0..3 {
            let a = bg[y0 * gw + x0][i] * (1.0 - tx) + bg[y0 * gw + x1][i] * tx;
            let b = bg[y1 * gw + x0][i] * (1.0 - tx) + bg[y1 * gw + x1][i] * tx;
            let b = (a * (1.0 - ty) + b * ty).max(16.0);
            p.0[i] = ((p.0[i] as f32 / b) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_warp_preserves_pixels() {
        let mut img = RgbaImage::from_pixel(20, 10, Rgba([255, 255, 255, 255]));
        img.put_pixel(5, 5, Rgba([0, 0, 0, 255]));
        let out = warp_perspective(&img, &Homography::identity(), 20, 10);
        assert_eq!(out.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert_eq!(out.get_pixel(6, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn enhance_flattens_gradient() {
        let img = RgbaImage::from_fn(64, 64, |x, _| {
            let v = (150 + x) as u8;
            Rgba([v, v, v, 255])
        });
        let out = enhance_photo(&img);
        let a = out.get_pixel(5, 30).0[0] as i32;
        let b = out.get_pixel(58, 30).0[0] as i32;
        assert!((a - b).abs() < 25, "{a} {b}");
        assert!(a > 220);
    }
}
