//! Derived images (decoded source, corrected image) and their GPU textures.

use std::sync::Arc;

use egui::{TextureHandle, Vec2};
use image::RgbaImage;
use plot2csv_core::project::{Correction, Project};

/// Images are processed at most this large (long side, px).
pub const MAX_WORK_SIDE: u32 = 3200;

#[derive(Default)]
pub struct Images {
    pub source: Option<Arc<RgbaImage>>,
    pub source_tex: Option<TextureHandle>,
    source_key: Option<(usize, bool)>,
    pub corrected: Option<Arc<RgbaImage>>,
    pub corrected_tex: Option<TextureHandle>,
    corrected_key: Option<String>,
    /// Grey version of the corrected image (for OCR / vision).
    pub corrected_gray: Option<Arc<image::GrayImage>>,
}

pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let mut rgba = img.to_rgba8();
    let long = rgba.width().max(rgba.height());
    if long > MAX_WORK_SIDE {
        let s = MAX_WORK_SIDE as f32 / long as f32;
        let w = ((rgba.width() as f32 * s).round() as u32).max(1);
        let h = ((rgba.height() as f32 * s).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle);
    }
    Ok(rgba)
}

fn make_texture(ctx: &egui::Context, name: &str, img: &RgbaImage) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side) as u32;
    let shown;
    let img = if img.width() > max || img.height() > max {
        let s = max as f32 / img.width().max(img.height()) as f32;
        shown = image::imageops::resize(
            img,
            ((img.width() as f32 * s) as u32).max(1),
            ((img.height() as f32 * s) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        &shown
    } else {
        img
    };
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    ctx.load_texture(name, ci, egui::TextureOptions::LINEAR)
}

fn correction_key(c: &Correction) -> String {
    if !c.enabled {
        return "none".into();
    }
    format!("{:?}{}x{}", c.quad, c.out_w, c.out_h)
}

impl Images {
    pub fn source_size(&self) -> Vec2 {
        self.source.as_ref().map(|i| Vec2::new(i.width() as f32, i.height() as f32)).unwrap_or(Vec2::ZERO)
    }

    pub fn corrected_size(&self) -> Vec2 {
        self.corrected.as_ref().map(|i| Vec2::new(i.width() as f32, i.height() as f32)).unwrap_or(Vec2::ZERO)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Bring the source image up to date with the project. Returns an error
    /// message if the stored image cannot be decoded.
    pub fn sync_source(&mut self, ctx: &egui::Context, project: &Project) -> Result<bool, String> {
        let Some(src) = &project.source else {
            if self.source.is_some() {
                self.clear();
            }
            return Ok(false);
        };
        let enhance = project.import.value.as_ref().map(|o| o.enhance).unwrap_or(false);
        let key = (src.bytes.len() ^ (src.bytes.first().copied().unwrap_or(0) as usize) << 3, enhance);
        if self.source_key == Some(key) && self.source.is_some() {
            return Ok(false);
        }
        let mut img = decode(&src.bytes)?;
        if enhance {
            img = plot2csv_vision::warp::enhance_photo(&img);
        }
        self.source_tex = Some(make_texture(ctx, "source", &img));
        self.source = Some(Arc::new(img));
        self.source_key = Some(key);
        self.corrected_key = None;
        Ok(true)
    }

    /// Recompute the corrected image if the correction changed.
    pub fn sync_corrected(&mut self, ctx: &egui::Context, project: &Project) -> bool {
        let Some(src) = self.source.clone() else {
            return false;
        };
        let corr = project.correction.value.clone().unwrap_or_else(|| Correction::identity(src.width(), src.height()));
        let key = correction_key(&corr);
        if self.corrected_key.as_deref() == Some(key.as_str()) && self.corrected.is_some() {
            return false;
        }
        let out = if corr.enabled {
            Arc::new(plot2csv_vision::warp::warp_perspective(&src, &corr.homography(), corr.out_w, corr.out_h))
        } else {
            src
        };
        self.corrected_tex = Some(make_texture(ctx, "corrected", &out));
        self.corrected_gray = Some(Arc::new(plot2csv_vision::raster::rgba_to_gray(&out)));
        self.corrected = Some(out);
        self.corrected_key = Some(key);
        true
    }

    /// Does the corrected image match the current correction?
    pub fn corrected_is_current(&self, project: &Project) -> bool {
        let Some(src) = &self.source else { return false };
        let corr = project.correction.value.clone().unwrap_or_else(|| Correction::identity(src.width(), src.height()));
        self.corrected.is_some() && self.corrected_key.as_deref() == Some(correction_key(&corr).as_str())
    }
}

/// Build an RGBA overlay texture from a bitmap (used for masks and colour filters).
pub fn bitmap_texture(ctx: &egui::Context, name: &str, w: usize, h: usize, pixel: impl Fn(usize) -> [u8; 4]) -> TextureHandle {
    let max = ctx.input(|i| i.max_texture_side);
    let step = w.max(h).div_ceil(max).max(1);
    let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
    let mut data = Vec::with_capacity(tw * th * 4);
    for ty in 0..th {
        for tx in 0..tw {
            let i = (ty * step).min(h - 1) * w + (tx * step).min(w - 1);
            data.extend_from_slice(&pixel(i));
        }
    }
    let ci = egui::ColorImage::from_rgba_unmultiplied([tw, th], &data);
    ctx.load_texture(name, ci, egui::TextureOptions::NEAREST)
}
