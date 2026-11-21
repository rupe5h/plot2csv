//! Port of the indra OCR pre/post-processing (det_post.py, rec_preproc.py and
//! the detect/recognize parts of pipeline.py, indra-ocr v1.1.0).

use crate::backend::{Model, OcrBackend, Tensor};
use image::GrayImage;
use serde::{Deserialize, Serialize};
use plot2csv_vision::raster::{component_stats, crop_gray, label_components, resize_gray};

pub const INPUT_MULTIPLE: usize = 32;
pub const OUT_STRIDE: f32 = 2.0;
pub const CORE_X_SHRINK: f32 = 0.15;
pub const ADAPTIVE_MIN_H: f32 = 9.0;
pub const ADAPTIVE_TARGET_H: f32 = 12.0;
pub const ADAPTIVE_MAX_SIDE: u32 = 2048;
pub const PAD_Y: f32 = 0.2;
pub const PAD_X: f32 = 0.3;
pub const WIDTH_MULTIPLE: usize = 8;
pub const MIN_BATCH_WIDTH: usize = 32;
pub const MIN_CONF: f32 = 0.5;

/// Settings read from indra's `meta.json`.
#[derive(Clone, Debug)]
pub struct IndraConfig {
    pub det_long_side: u32,
    pub rec_height: u32,
    pub rec_window: u32,
    /// Token strings; class `i + 1` is `chars[i]`, class 0 is the CTC blank.
    pub chars: Vec<String>,
}

impl IndraConfig {
    pub fn from_meta_json(text: &str) -> anyhow::Result<Self> {
        let v: serde_json::Value = serde_json::from_str(text)?;
        let vocab_v = &v["vocab"];
        // `vocab` is stored as a JSON string holding {"blank":0,"chars":[...]}.
        let vocab: serde_json::Value = match vocab_v {
            serde_json::Value::String(s) => serde_json::from_str(s)?,
            other => other.clone(),
        };
        let chars = vocab["chars"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("meta.json: vocab.chars missing"))?
            .iter()
            .map(|c| c.as_str().unwrap_or("").to_string())
            .collect();
        Ok(Self {
            det_long_side: v["det_long_side"].as_u64().unwrap_or(1024) as u32,
            rec_height: v["rec_height"].as_u64().unwrap_or(32) as u32,
            rec_window: v["rec_window"].as_u64().unwrap_or(1) as u32,
            chars,
        })
    }

    pub fn decode(&self, ids: &[i64]) -> String {
        ids.iter()
            .filter(|&&i| i > 0 && (i as usize) <= self.chars.len())
            .map(|&i| self.chars[i as usize - 1].as_str())
            .collect()
    }
}

/// A recognised text line in page pixels.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextLine {
    /// x0, y0, x1, y1
    pub bbox: [f32; 4],
    pub text: String,
    pub conf: f32,
    /// Each decoded token with its page x position (CTC peak).
    pub chars: Vec<(String, f32)>,
}

/// Detector input: ink map padded to a multiple of 32. Returns (data, H, W, scale).
pub fn page_to_input(gray: &GrayImage, long_side: u32) -> (Vec<f32>, usize, usize, f32) {
    let (h, w) = (gray.height() as f32, gray.width() as f32);
    let scale = long_side as f32 / h.max(w);
    let nh = ((h * scale).round() as u32).max(1);
    let nw = ((w * scale).round() as u32).max(1);
    let resized = if (nh, nw) != (gray.height(), gray.width()) {
        resize_gray(gray, nw, nh, scale < 1.0)
    } else {
        gray.clone()
    };
    let ph = (nh as usize).div_ceil(INPUT_MULTIPLE) * INPUT_MULTIPLE;
    let pw = (nw as usize).div_ceil(INPUT_MULTIPLE) * INPUT_MULTIPLE;
    let mut out = vec![0f32; ph * pw];
    let raw = resized.as_raw();
    for y in 0..nh as usize {
        for x in 0..nw as usize {
            out[y * pw + x] = 1.0 - raw[y * nw as usize + x] as f32 / 255.0;
        }
    }
    (out, ph, pw, scale)
}

/// Decode the detector output `[3, h, w]` into boxes (input pixels): x0,y0,x1,y1,score.
pub fn decode_det(out: &[f32], h: usize, w: usize, thresh: f32, min_area: usize) -> Vec<[f32; 5]> {
    let plane = h * w;
    let logit = &out[..plane];
    let top_d = &out[plane..2 * plane];
    let bot_d = &out[2 * plane..3 * plane];
    let prob: Vec<f32> = logit.iter().map(|&l| 1.0 / (1.0 + (-l).exp())).collect();
    let mask: Vec<bool> = prob.iter().map(|&p| p > thresh).collect();
    let (labels, n) = label_components(&mask, w, h, false);
    if n == 0 {
        return Vec::new();
    }
    let stats = component_stats(&labels, n, w);
    let mut sum_top = vec![0f64; n as usize + 1];
    let mut sum_bot = vec![0f64; n as usize + 1];
    let mut sum_sc = vec![0f64; n as usize + 1];
    for (i, &l) in labels.iter().enumerate() {
        if l == 0 {
            continue;
        }
        let ys = ((i / w) as f32 + 0.5) * OUT_STRIDE;
        sum_top[l as usize] += (ys - top_d[i]) as f64;
        sum_bot[l as usize] += (ys + bot_d[i]) as f64;
        sum_sc[l as usize] += prob[i] as f64;
    }
    let mut boxes = Vec::new();
    for i in 1..=n as usize {
        let st = &stats[i];
        if st.area < min_area {
            continue;
        }
        let cnt = st.area.max(1) as f64;
        let y0 = (sum_top[i] / cnt) as f32;
        let y1 = (sum_bot[i] / cnt) as f32;
        let lh = (y1 - y0).max(1.0);
        let ex = (CORE_X_SHRINK * lh).min(0.5 * lh);
        let x0 = st.left as f32 * OUT_STRIDE - ex;
        let x1 = (st.left + st.width()) as f32 * OUT_STRIDE + ex;
        boxes.push([x0, y0, x1, y1, (sum_sc[i] / cnt) as f32]);
    }
    boxes
}

fn percentiles(vals: &mut [u8], qs: &[f64]) -> Vec<f64> {
    let n = vals.len();
    let mut hist = [0usize; 256];
    for &v in vals.iter() {
        hist[v as usize] += 1;
    }
    let mut cum = [0usize; 256];
    let mut c = 0;
    for i in 0..256 {
        c += hist[i];
        cum[i] = c;
    }
    // searchsorted(cum, k, side="right"): first index with cum > k
    let ss = |k: usize| -> f64 { cum.iter().position(|&c| c > k).unwrap_or(255) as f64 };
    qs.iter()
        .map(|&q| {
            let x = q * (n as f64 - 1.0);
            let i = x.floor() as usize;
            let g = x - i as f64;
            let lo = ss(i);
            let hi = ss((i + 1).min(n - 1));
            let d = hi - lo;
            if g >= 0.5 {
                hi - d * (1.0 - g)
            } else {
                lo + d * g
            }
        })
        .collect()
}

/// Light text on a dark ground?
pub fn is_dark_line(gray: &GrayImage, b: &[f32; 4]) -> bool {
    let (w, h) = (gray.width() as i64, gray.height() as i64);
    let x0 = (b[0] as i64).max(0);
    let y0 = (b[1] as i64).max(0);
    let x1 = (b[2] as i64 + 1).min(w);
    let y1 = (b[3] as i64 + 1).min(h);
    if x1 <= x0 || y1 <= y0 {
        return false;
    }
    let mut vals = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            vals.push(gray.get_pixel(x as u32, y as u32).0[0]);
        }
    }
    let p = percentiles(&mut vals, &[0.02, 0.5, 0.98]);
    let (lo, med, hi) = (p[0], p[1], p[2]);
    hi - med > 2.0 * (med - lo) + 8.0
}

/// A recogniser crop: ink map `[height*window, W]` plus geometry needed to map
/// timesteps back to page x.
pub struct LineCrop {
    pub data: Vec<f32>,
    pub height: usize,
    pub width: usize,
    /// left edge of the crop in page px and its width in page px
    pub page_x0: f32,
    pub page_w: f32,
}

pub fn crop_line(gray: &GrayImage, b: &[f32; 4], height: u32, window: u32, dark: bool) -> LineCrop {
    let pad_y = (window as f32 * (1.0 + 2.0 * PAD_Y) - 1.0) / 2.0;
    let height = (height * window) as usize;
    let (x0, y0, x1, y1) = (b[0], b[1], b[2], b[3]);
    let h = (y1 - y0).max(1.0);
    let (xx0, xx1) = (x0 - PAD_X * h, x1 + PAD_X * h);
    let (yy0, yy1) = (y0 - pad_y * h, y1 + pad_y * h);
    let ix0 = xx0.floor() as i64;
    let iy0 = yy0.floor() as i64;
    let ix1 = xx1.ceil() as i64;
    let iy1 = yy1.ceil() as i64;
    let cw = (ix1 - ix0).max(1) as u32;
    let ch = (iy1 - iy0).max(1) as u32;
    let crop = crop_gray(gray, ix0, iy0, cw, ch, if dark { 0 } else { 255 });
    let out_w = ((cw as f32 * height as f32 / ch as f32).round() as usize).clamp(WIDTH_MULTIPLE, 4096);
    let resized = resize_gray(&crop, out_w as u32, height as u32, ch as usize > height);
    let data = resized
        .as_raw()
        .iter()
        .map(|&v| {
            let ink = v as f32 / 255.0;
            if dark {
                ink
            } else {
                1.0 - ink
            }
        })
        .collect();
    LineCrop { data, height, width: out_w, page_x0: ix0 as f32, page_w: cw as f32 }
}

/// Group line indices into batches of similar width (indra `width_batches`).
pub fn width_batches(widths: &[usize], max_batch: usize) -> Vec<Vec<usize>> {
    let (max_ratio, min_width, max_area) = (1.3f32, 64usize, 64 * 640usize);
    let mut order: Vec<usize> = (0..widths.len()).collect();
    order.sort_by_key(|&i| widths[i]);
    let mut batches: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let wi = widths[i].max(min_width);
        if let Some(last) = batches.last_mut() {
            let w0 = widths[last[0]].max(min_width);
            if last.len() < max_batch
                && wi as f32 <= max_ratio * w0 as f32
                && (last.len() + 1) * wi <= max_area
            {
                last.push(i);
                continue;
            }
        }
        batches.push(vec![i]);
    }
    batches
}

/// Detector long side that keeps a crop near its native scale (the detector is
/// tuned for ~12 px text lines); big pages use the model default.
pub fn native_long_side(cfg: &IndraConfig, gray: &GrayImage) -> u32 {
    let long = gray.width().max(gray.height());
    if long >= cfg.det_long_side {
        cfg.det_long_side
    } else {
        (long.div_ceil(32) * 32).clamp(256, cfg.det_long_side)
    }
}

/// Detect text lines; boxes are returned in page pixels.
pub async fn detect(backend: &dyn OcrBackend, cfg: &IndraConfig, gray: &GrayImage) -> anyhow::Result<Vec<[f32; 5]>> {
    detect_at(backend, cfg, gray, cfg.det_long_side).await
}

/// Detect with an explicit detector long side.
pub async fn detect_at(backend: &dyn OcrBackend, _cfg: &IndraConfig, gray: &GrayImage, base_side: u32) -> anyhow::Result<Vec<[f32; 5]>> {
    let run = |long_side: u32| async move {
        let (x, ph, pw, s) = page_to_input(gray, long_side);
        let out = backend
            .run(Model::Det, vec![("x", Tensor::f32(vec![1, 1, ph, pw], x))], &["out"])
            .await?;
        let t = &out[0];
        let (c, h, w) = (t.dims[1], t.dims[2], t.dims[3]);
        anyhow::ensure!(c >= 3, "det output has {c} channels");
        let boxes = decode_det(&t.as_f32()?[..3 * h * w], h, w, 0.5, 2);
        anyhow::Ok((boxes, s))
    };
    let (mut boxes, mut s) = run(base_side).await?;
    if boxes.len() >= 8 {
        let mut hs: Vec<f32> = boxes.iter().map(|b| b[3] - b[1]).collect();
        hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = median_sorted(&hs);
        if med < ADAPTIVE_MIN_H {
            let ls = ((ADAPTIVE_MAX_SIDE as f32).min(base_side as f32 * ADAPTIVE_TARGET_H / med.max(1.0)) as u32) / 32 * 32;
            if ls > base_side {
                (boxes, s) = run(ls).await?;
            }
        }
    }
    let (w, h) = (gray.width() as f32, gray.height() as f32);
    for b in boxes.iter_mut() {
        for v in b.iter_mut().take(4) {
            *v /= s;
        }
        b[0] = b[0].clamp(0.0, w);
        b[2] = b[2].clamp(0.0, w);
        b[1] = b[1].clamp(0.0, h);
        b[3] = b[3].clamp(0.0, h);
    }
    Ok(boxes)
}

fn median_sorted(v: &[f32]) -> f32 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Recognise the given line boxes (page px).
pub async fn recognize(
    backend: &dyn OcrBackend,
    cfg: &IndraConfig,
    gray: &GrayImage,
    boxes: &[[f32; 4]],
) -> anyhow::Result<Vec<TextLine>> {
    let crops: Vec<LineCrop> = boxes
        .iter()
        .map(|b| crop_line(gray, b, cfg.rec_height, cfg.rec_window.max(1), is_dark_line(gray, b)))
        .collect();
    let mut out: Vec<TextLine> = boxes
        .iter()
        .map(|b| TextLine { bbox: *b, text: String::new(), conf: 0.0, chars: Vec::new() })
        .collect();
    let widths: Vec<usize> = crops.iter().map(|c| c.width).collect();
    for batch in width_batches(&widths, 64) {
        let hh = crops[batch[0]].height;
        let wm = batch.iter().map(|&i| crops[i].width).max().unwrap_or(0).max(MIN_BATCH_WIDTH);
        let wm = wm.div_ceil(WIDTH_MULTIPLE) * WIDTH_MULTIPLE;
        let mut arr = vec![0f32; batch.len() * hh * wm];
        let mut ws = Vec::with_capacity(batch.len());
        for (k, &i) in batch.iter().enumerate() {
            let c = &crops[i];
            for y in 0..hh {
                let dst = k * hh * wm + y * wm;
                arr[dst..dst + c.width].copy_from_slice(&c.data[y * c.width..(y + 1) * c.width]);
            }
            ws.push(c.width as i64);
        }
        let res = backend
            .run(
                Model::Rec,
                vec![
                    ("x", Tensor::f32(vec![batch.len(), 1, hh, wm], arr)),
                    ("widths", Tensor::i64(vec![batch.len()], ws)),
                ],
                &["ids", "conf", "lengths"],
            )
            .await?;
        let ids = res[0].as_i64()?;
        let pmax = res[1].as_f32()?;
        let lengths = res[2].as_i64()?;
        let t_len = res[0].dims[1];
        let stride = wm as f32 / t_len.max(1) as f32;
        for (k, &i) in batch.iter().enumerate() {
            let n = (lengths[k].max(0) as usize).min(t_len);
            let seq = &ids[k * t_len..k * t_len + n];
            let pm = &pmax[k * t_len..k * t_len + n];
            let mut kept_ids = Vec::new();
            let mut kept_t = Vec::new();
            let mut conf_sum = 0f32;
            for t in 0..n {
                if seq[t] != 0 && (t == 0 || seq[t] != seq[t - 1]) {
                    kept_ids.push(seq[t]);
                    kept_t.push(t);
                    conf_sum += pm[t];
                }
            }
            let conf = if kept_ids.is_empty() { 0.0 } else { conf_sum / kept_ids.len() as f32 };
            let c = &crops[i];
            let f = c.page_w / c.width as f32;
            let line = &mut out[i];
            line.conf = conf;
            if conf >= MIN_CONF {
                line.text = cfg.decode(&kept_ids);
                line.chars = kept_ids
                    .iter()
                    .zip(&kept_t)
                    .map(|(&id, &t)| {
                        (cfg.decode(&[id]), c.page_x0 + (t as f32 + 0.5) * stride * f)
                    })
                    .collect();
            }
        }
    }
    Ok(out)
}

/// Full page OCR: detect then recognise, lines sorted top-to-bottom, left-to-right.
pub async fn ocr_page(backend: &dyn OcrBackend, cfg: &IndraConfig, gray: &GrayImage) -> anyhow::Result<Vec<TextLine>> {
    let det = detect(backend, cfg, gray).await?;
    let boxes: Vec<[f32; 4]> = det.iter().map(|b| [b[0], b[1], b[2], b[3]]).collect();
    let mut lines = recognize(backend, cfg, gray, &boxes).await?;
    lines.sort_by(|a, b| {
        let ka = (a.bbox[1] / 4.0).round() as i64;
        let kb = (b.bbox[1] / 4.0).round() as i64;
        ka.cmp(&kb).then(a.bbox[0].partial_cmp(&b.bbox[0]).unwrap())
    });
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_matches_numpy_linear() {
        let mut v: Vec<u8> = (0..=100).collect();
        let p = percentiles(&mut v, &[0.02, 0.5, 0.98]);
        assert!((p[0] - 2.0).abs() < 1e-9);
        assert!((p[1] - 50.0).abs() < 1e-9);
        assert!((p[2] - 98.0).abs() < 1e-9);
    }

    #[test]
    fn page_input_is_padded_multiple_of_32() {
        let g = GrayImage::from_pixel(100, 50, image::Luma([255]));
        let (d, h, w, s) = page_to_input(&g, 1024);
        assert_eq!((h % 32, w % 32), (0, 0));
        assert_eq!(w, 1024);
        assert!((s - 10.24).abs() < 1e-4);
        assert!(d.iter().all(|&v| v.abs() < 1e-6));
    }

    #[test]
    fn crop_window_three_has_height_96() {
        let g = GrayImage::from_pixel(200, 100, image::Luma([255]));
        let c = crop_line(&g, &[10.0, 40.0, 110.0, 60.0], 32, 3, false);
        assert_eq!(c.height, 96);
        // same text scale as window 1: width = cw * 32 / (1.4 h)
        let c1 = crop_line(&g, &[10.0, 40.0, 110.0, 60.0], 32, 1, false);
        assert!((c.width as i64 - c1.width as i64).abs() <= 1);
    }

    #[test]
    fn decode_single_component() {
        let (h, w) = (4, 6);
        let mut out = vec![-10f32; 3 * h * w];
        for x in 1..5 {
            out[w + x] = 10.0; // row 1
            out[h * w + w + x] = 3.0; // top dist
            out[2 * h * w + w + x] = 3.0; // bottom dist
        }
        let b = decode_det(&out, h, w, 0.5, 2);
        assert_eq!(b.len(), 1);
        let ys = 1.5 * 2.0;
        assert!((b[0][1] - (ys - 3.0)).abs() < 1e-4);
        assert!((b[0][3] - (ys + 3.0)).abs() < 1e-4);
    }

    #[test]
    fn batches_group_similar_widths() {
        let b = width_batches(&[100, 110, 400, 105], 64);
        assert_eq!(b.len(), 2);
    }
}
