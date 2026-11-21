//! Automatic rotation / perspective correction.
//!
//! Long straight lines are grouped into a near-horizontal and a near-vertical
//! family. The vanishing point of each family is estimated robustly: chords of
//! the plotted data (sine flanks, decaying tails, peaks) also look like lines to
//! the Hough transform, so each family keeps only the largest consistent set
//! (by length², favouring straight strokes) before a least-squares fit. A family
//! whose lines a single direction explains is treated as parallel. A homography
//! sending both vanishing points to infinity (and the families to the x / y
//! directions) rectifies the image. With parallel families this reduces to a
//! deskew (plus shear removal).

use image::GrayImage;
use nalgebra::{Matrix3, SymmetricEigen, Vector3};
use plot2csv_core::geom::{Homography, Pt};
use plot2csv_core::project::Correction;

use crate::lines::{detect_ink, dominant_skew, hough_segments, refine_segment, Ink, Segment};
use crate::raster::downscale_to;

pub struct CorrectionResult {
    pub correction: Correction,
    pub rotation_deg: f64,
    pub note: String,
    /// Detected line segments in source px (for display / debugging).
    pub h_lines: Vec<(Pt, Pt)>,
    pub v_lines: Vec<(Pt, Pt)>,
}

const WORK_SIDE: u32 = 1100;

fn none(w: u32, h: u32, note: &str) -> CorrectionResult {
    CorrectionResult { correction: Correction::identity(w, h), rotation_deg: 0.0, note: note.into(), h_lines: vec![], v_lines: vec![] }
}

/// Weighted least-squares vanishing point of a family of lines (homogeneous, normalised coords).
fn vanishing_point(lines: &[([f64; 3], f64)]) -> Vector3<f64> {
    let mut m = Matrix3::<f64>::zeros();
    for (l, w) in lines {
        let v = Vector3::new(l[0], l[1], l[2]);
        m += v * v.transpose() * *w;
    }
    let e = SymmetricEigen::new(m);
    let mut best = 0;
    for i in 1..3 {
        if e.eigenvalues[i] < e.eigenvalues[best] {
            best = i;
        }
    }
    e.eigenvectors.column(best).into_owned()
}

/// RANSAC inlier tolerance: endpoint distance (work px) from the line joining
/// the segment's midpoint to the vanishing point.
const INLIER_PX: f64 = 2.0;
/// A family is parallel when one direction fits every inlier this well (work px).
const PARALLEL_PX: f64 = 1.0;
/// Vanishing points closer than this to the image centre (in long-side units)
/// would mean a wildly oblique view; they come from crossing curve chords.
const MIN_VP_DIST: f64 = 1.0;
/// Vanishing points farther than this are indistinguishable from parallel lines.
const MAX_VP_DIST: f64 = 25.0;

/// A candidate line of one family, in work px, with its weight.
#[derive(Clone, Copy)]
struct FamLine {
    seg: Segment,
    w: f64,
}

/// Refine Hough segments onto their stroke centres and weight them by length²,
/// discounting strokes that bend (chords of curves). Segments that cannot be
/// refined (edges such as a page border, very thick strokes) keep their Hough
/// geometry at half weight.
fn prepare(g: &GrayImage, ink: &Ink, segs: &[Segment]) -> Vec<FamLine> {
    segs.iter()
        .map(|s| match refine_segment(g, ink, s) {
            Some(r) => {
                let straight = ((0.8 - r.rms) / 0.35).clamp(0.1, 1.0);
                FamLine { seg: r.seg, w: r.seg.len().powi(2) * straight }
            }
            None => FamLine { seg: *s, w: s.len().powi(2) * 0.5 },
        })
        .collect()
}

/// The fitted family: inlier lines and the vanishing point (homogeneous,
/// normalised coordinates; z = 0 for parallel lines).
struct FamilyFit {
    lines: Vec<Segment>,
    vp: Vector3<f64>,
    persp: bool,
}

/// Robust vanishing point of one line family. `n` maps work px to normalised
/// coordinates (centred, divided by `sc`, the long side).
fn fit_family(cands: &[FamLine], n: &Homography, sc: f64) -> Option<FamilyFit> {
    if cands.is_empty() {
        return None;
    }
    let hom = |p: Pt| {
        let q = n.apply(p);
        Vector3::new(q.x, q.y, 1.0)
    };
    let norm_lines: Vec<(Vector3<f64>, Vector3<f64>, Vector3<f64>)> = cands
        .iter()
        .map(|c| {
            let (a, b) = (hom(c.seg.a), hom(c.seg.b));
            let l = a.cross(&b);
            let k = (l.x * l.x + l.y * l.y).sqrt().max(1e-12);
            (a, b, l / k)
        })
        .collect();
    // Endpoint distance (work px) from the line through the segment midpoint and `vp`.
    let dev = |i: usize, vp: &Vector3<f64>| -> f64 {
        let (a, b, _) = &norm_lines[i];
        let m = (a + b) / 2.0;
        let l = m.cross(vp);
        let k = (l.x * l.x + l.y * l.y).sqrt();
        if k < 1e-12 {
            return f64::INFINITY;
        }
        (l.dot(a) / k).abs() * sc
    };
    let plausible = |vp: &Vector3<f64>| {
        let k = vp.norm();
        if k < 1e-12 {
            return false;
        }
        vp.z.abs() < 1e-12 || (vp.x / vp.z).hypot(vp.y / vp.z) >= MIN_VP_DIST
    };
    let inliers = |vp: &Vector3<f64>, tol: f64| -> Vec<usize> { (0..cands.len()).filter(|&i| dev(i, vp) <= tol).collect() };

    // Hypotheses: each line's own direction (parallel family) and each pair's intersection.
    let mut best: Option<(f64, Vec<usize>)> = None;
    for i in 0..cands.len() {
        for j in i..cands.len() {
            let vp = if i == j {
                let d = cands[i].seg.b.sub(cands[i].seg.a);
                Vector3::new(d.x, d.y, 0.0)
            } else {
                norm_lines[i].2.cross(&norm_lines[j].2)
            };
            if !plausible(&vp) {
                continue;
            }
            let inl = inliers(&vp, INLIER_PX);
            let score: f64 = inl.iter().map(|&k| cands[k].w).sum();
            if best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, inl));
            }
        }
    }
    let (_, inl) = best?;

    // Parallel model: weighted mean direction (doubled angles) of the inliers.
    let (mut sx, mut sy) = (0.0, 0.0);
    for &k in &inl {
        let a = cands[k].seg.angle_deg().to_radians() * 2.0;
        sx += a.cos() * cands[k].w;
        sy += a.sin() * cands[k].w;
    }
    let ang = sy.atan2(sx) / 2.0;
    let par = Vector3::new(ang.cos(), ang.sin(), 0.0);
    let parallel_ok = inl.iter().all(|&k| dev(k, &par) <= PARALLEL_PX);

    let (vp, inl) = if inl.len() >= 2 && !parallel_ok {
        let weighted: Vec<([f64; 3], f64)> = inl.iter().map(|&k| (norm_lines[k].2.into(), cands[k].w)).collect();
        let ls = vanishing_point(&weighted);
        // Re-collect inliers around the refined point (it may gain or drop a line).
        let inl2 = inliers(&ls, INLIER_PX);
        if inl2.len() >= 2 {
            (ls, inl2)
        } else {
            (ls, inl)
        }
    } else {
        (par, inl)
    };
    let persp = vp.z.abs() > 1e-9 && (vp.x / vp.z).hypot(vp.y / vp.z) < MAX_VP_DIST;
    let vp = if vp.z.abs() > 1e-9 && !persp { par } else { vp };
    Some(FamilyFit { lines: inl.iter().map(|&k| cands[k].seg).collect(), vp, persp })
}

pub fn detect_correction(src: &GrayImage) -> CorrectionResult {
    let (sw, sh) = (src.width(), src.height());
    let (g, s) = downscale_to(src, WORK_SIDE);
    let ink = detect_ink(&g, 12, 22.0);
    let (w, h) = (g.width() as f64, g.height() as f64);
    if ink.count() < 200 {
        return none(sw, sh, "No straight lines found; no correction proposed.");
    }
    // Coarse orientation of both families, then fine Hough around them.
    let (hs, _) = dominant_skew(&ink, false, -45.0, 45.0, 0.5);
    let (hs, _) = dominant_skew(&ink, false, hs - 0.6, hs + 0.6, 0.05);
    let (vs, _) = dominant_skew(&ink, true, hs - 15.0, hs + 15.0, 0.5);
    let (vs, _) = dominant_skew(&ink, true, vs - 0.6, vs + 0.6, 0.05);
    let min_h = 0.18 * w;
    let min_v = 0.18 * h;
    let hl_raw: Vec<Segment> = hough_segments(&ink, hs, 15.0, 0.1, min_h, 25);
    let vl_raw: Vec<Segment> = hough_segments(&ink, vs + 90.0, 15.0, 0.1, min_v, 25);
    if hl_raw.is_empty() && vl_raw.is_empty() {
        return none(sw, sh, "No long straight lines found; no correction proposed.");
    }

    // Normalised coordinates: centre at 0, scale by the long side.
    let sc = w.max(h);
    let n = Homography { m: [[1.0 / sc, 0.0, -w / 2.0 / sc], [0.0, 1.0 / sc, -h / 2.0 / sc], [0.0, 0.0, 1.0]] };

    // Robust per-family fit: drops chords of the data curves.
    let fh = fit_family(&prepare(&g, &ink, &hl_raw), &n, sc);
    let fv = fit_family(&prepare(&g, &ink, &vl_raw), &n, sc);
    let hl: Vec<Segment> = fh.as_ref().map(|f| f.lines.clone()).unwrap_or_default();
    let vl: Vec<Segment> = fv.as_ref().map(|f| f.lines.clone()).unwrap_or_default();
    let to_src = |p: Pt| p.scale(1.0 / s as f64);
    let h_lines: Vec<(Pt, Pt)> = hl.iter().map(|l| (to_src(l.a), to_src(l.b))).collect();
    let v_lines: Vec<(Pt, Pt)> = vl.iter().map(|l| (to_src(l.a), to_src(l.b))).collect();

    // Directions used when a family has no lines.
    let dir_h = |t: f64| Vector3::new(t.to_radians().cos(), t.to_radians().sin(), 0.0);
    let vh = fh.as_ref().map(|f| f.vp).unwrap_or_else(|| dir_h(hs));
    let vv = fv.as_ref().map(|f| f.vp).unwrap_or_else(|| dir_h(vs + 90.0));
    let persp = fh.as_ref().is_some_and(|f| f.persp) || fv.as_ref().is_some_and(|f| f.persp);

    // Projective part: send the vanishing line to infinity.
    let line_inf = vh.cross(&vv);
    let hp = if persp && line_inf.z.abs() > 1e-12 {
        let l = line_inf / line_inf.z;
        Matrix3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, l.x, l.y, 1.0)
    } else {
        Matrix3::identity()
    };
    // Directions after Hp.
    let dh = hp * vh;
    let dv = hp * vv;
    let mut dh2 = nalgebra::Vector2::new(dh.x, dh.y);
    let mut dv2 = nalgebra::Vector2::new(dv.x, dv.y);
    if dh2.norm() < 1e-12 || dv2.norm() < 1e-12 {
        return none(sw, sh, "Could not estimate the orientation; no correction proposed.");
    }
    dh2 /= dh2.norm();
    dv2 /= dv2.norm();
    // Orient: x increases to the right, y downwards.
    if dh2.x < 0.0 {
        dh2 = -dh2;
    }
    if dv2.y < 0.0 {
        dv2 = -dv2;
    }
    let basis = nalgebra::Matrix2::from_columns(&[dh2, dv2]);
    let Some(a_lin) = basis.try_inverse() else {
        return none(sw, sh, "Line families are degenerate; no correction proposed.");
    };
    let a3 = Matrix3::new(a_lin[(0, 0)], a_lin[(0, 1)], 0.0, a_lin[(1, 0)], a_lin[(1, 1)], 0.0, 0.0, 0.0, 1.0);
    let rect = a3 * hp; // normalised source -> rectified (unknown scale)
    let rect_h = Homography { m: [[rect[(0, 0)], rect[(0, 1)], rect[(0, 2)]], [rect[(1, 0)], rect[(1, 1)], rect[(1, 2)]], [rect[(2, 0)], rect[(2, 1)], rect[(2, 2)]]] };

    // Full transform in source px: S · rect · N · (1/s scaling to work px).
    let to_work = Homography { m: [[s as f64, 0.0, 0.0], [0.0, s as f64, 0.0], [0.0, 0.0, 1.0]] };
    let base = to_work.then(&n).then(&rect_h);
    // Rotation magnitude for the report: angle of the horizontal family.
    let rotation_deg = dh2.y.atan2(dh2.x).to_degrees();
    let shear_deg = (90.0 - dh2.angle(&dv2).to_degrees()).abs();

    // Scale x and y so that line lengths are preserved, then fit the image corners.
    let corners = [Pt::new(0.0, 0.0), Pt::new(sw as f64, 0.0), Pt::new(sw as f64, sh as f64), Pt::new(0.0, sh as f64)];
    let scale_for = |segs: &[Segment], horiz: bool| -> Option<f64> {
        let (mut num, mut den) = (0.0, 0.0);
        for l in segs {
            let a = base.apply(to_src(l.a));
            let b = base.apply(to_src(l.b));
            let d = if horiz { (b.x - a.x).abs() } else { (b.y - a.y).abs() };
            num += l.len() / s as f64;
            den += d;
        }
        (den > 1e-12).then_some(num / den)
    };
    let tc: Vec<Pt> = corners.iter().map(|p| base.apply(*p)).collect();
    let span_x = tc.iter().map(|p| p.x).fold(f64::MIN, f64::max) - tc.iter().map(|p| p.x).fold(f64::MAX, f64::min);
    let span_y = tc.iter().map(|p| p.y).fold(f64::MIN, f64::max) - tc.iter().map(|p| p.y).fold(f64::MAX, f64::min);
    let kx = scale_for(&hl, true).unwrap_or(sw as f64 / span_x.max(1e-12));
    let ky = scale_for(&vl, false).unwrap_or(sh as f64 / span_y.max(1e-12));
    let scaled = base.then(&Homography { m: [[kx, 0.0, 0.0], [0.0, ky, 0.0], [0.0, 0.0, 1.0]] });

    // Output rectangle: bounding box of the content region (the detected lines'
    // extent grown by 8% and clipped to the transformed image), so warped
    // margins outside the page stay small.
    let mut pts: Vec<Pt> = hl.iter().chain(vl.iter()).flat_map(|l| [to_src(l.a), to_src(l.b)]).map(|p| scaled.apply(p)).collect();
    let tc2: Vec<Pt> = corners.iter().map(|p| scaled.apply(*p)).collect();
    if pts.is_empty() {
        pts = tc2.clone();
    }
    let bb = |v: &[Pt]| {
        (
            v.iter().map(|p| p.x).fold(f64::MAX, f64::min),
            v.iter().map(|p| p.y).fold(f64::MAX, f64::min),
            v.iter().map(|p| p.x).fold(f64::MIN, f64::max),
            v.iter().map(|p| p.y).fold(f64::MIN, f64::max),
        )
    };
    let (cx0, cy0, cx1, cy1) = bb(&pts);
    let (ix0, iy0, ix1, iy1) = bb(&tc2);
    let mx = (cx1 - cx0) * 0.12 + 20.0;
    let my = (cy1 - cy0) * 0.12 + 20.0;
    // Prefer the whole image when perspective is mild (screenshots, scans).
    let (ox0, oy0, ox1, oy1) = if persp {
        ((cx0 - mx).max(ix0), (cy0 - my).max(iy0), (cx1 + mx).min(ix1), (cy1 + my).min(iy1))
    } else {
        (ix0, iy0, ix1, iy1)
    };
    let (out_w, out_h) = ((ox1 - ox0).round().max(16.0), (oy1 - oy0).round().max(16.0));
    // Guard against absurd outputs.
    if out_w > 3.0 * sw as f64 || out_h > 3.0 * sh as f64 {
        return none(sw, sh, "The detected lines gave an implausible correction; none proposed.");
    }
    let final_h = scaled.then(&Homography::translation(-ox0, -oy0));
    let inv = final_h.inverse().unwrap_or_default();
    let quad = [
        inv.apply(Pt::new(0.0, 0.0)),
        inv.apply(Pt::new(out_w, 0.0)),
        inv.apply(Pt::new(out_w, out_h)),
        inv.apply(Pt::new(0.0, out_h)),
    ];
    let n_lines = hl.len() + vl.len();
    let total_len: f64 = hl.iter().chain(vl.iter()).map(|l| l.len()).sum::<f64>() / (w + h);
    let confidence = (0.35 + 0.1 * n_lines.min(4) as f64 + 0.15 * total_len.min(2.0)).min(0.99);

    let mild = rotation_deg.abs() < 0.25 && shear_deg < 0.6 && !persp;
    let correction = Correction {
        enabled: !mild,
        quad: if mild { corners } else { quad },
        out_w: if mild { sw } else { out_w as u32 },
        out_h: if mild { sh } else { out_h as u32 },
        method: if mild {
            "none".into()
        } else if persp {
            "perspective".into()
        } else {
            "deskew".into()
        },
        confidence,
    };
    let note = if mild {
        format!("The image is already straight (rotation {rotation_deg:.2}°). No correction needed.")
    } else if persp {
        format!(
            "Perspective detected from {} horizontal and {} vertical lines (rotation {rotation_deg:.1}°, confidence {:.0}%).",
            hl.len(),
            vl.len(),
            confidence * 100.0
        )
    } else {
        format!("Rotation {rotation_deg:.2}°, shear {shear_deg:.2}° from {n_lines} lines (confidence {:.0}%).", confidence * 100.0)
    };
    CorrectionResult { correction, rotation_deg, note, h_lines, v_lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warp::warp_perspective;
    use image::{Luma, Rgba, RgbaImage};

    /// A synthetic plot: frame, ticks and a curve.
    fn synthetic_plot(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let (l, t, r, b) = (80, 50, w - 40, h - 70);
        for x in l..=r {
            for k in 0..2 {
                img.put_pixel(x, t + k, Rgba([0, 0, 0, 255]));
                img.put_pixel(x, b + k, Rgba([0, 0, 0, 255]));
            }
        }
        for y in t..=b {
            for k in 0..2 {
                img.put_pixel(l + k, y, Rgba([0, 0, 0, 255]));
                img.put_pixel(r + k, y, Rgba([0, 0, 0, 255]));
            }
        }
        for i in 0..200 {
            let x = l + 5 + i * (r - l - 10) / 200;
            let y = ((t + b) / 2) as f64 + 60.0 * ((i as f64) / 20.0).sin();
            img.put_pixel(x, y as u32, Rgba([220, 50, 50, 255]));
        }
        img
    }

    fn gray(img: &RgbaImage) -> GrayImage {
        crate::raster::rgba_to_gray(img)
    }

    /// Paint a thick polyline through `pts`.
    fn stroke(img: &mut RgbaImage, pts: &[(f64, f64)], r: f64, c: Rgba<u8>) {
        for w in pts.windows(2) {
            let (a, b) = (Pt::new(w[0].0, w[0].1), Pt::new(w[1].0, w[1].1));
            let n = (a.dist(b) * 2.0).ceil().max(1.0) as usize;
            for k in 0..=n {
                let p = a.lerp(b, k as f64 / n as f64);
                let ri = r.ceil() as i64;
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        let (x, y) = (p.x.round() as i64 + dx, p.y.round() as i64 + dy);
                        if ((dx * dx + dy * dy) as f64) <= r * r && x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                            img.put_pixel(x as u32, y as u32, c);
                        }
                    }
                }
            }
        }
    }

    /// A straight framed plot whose data are steep, growing sine waves and their
    /// envelopes: long nearly straight chords at ±5–15° that used to be taken
    /// for converging frame lines.
    fn growing_sines_plot() -> RgbaImage {
        let (w, h) = (1000u32, 750u32);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let (l, t, r, b) = (150.0, 40.0, 960.0, 665.0);
        let frame = [(l, t), (r, t), (r, b), (l, b), (l, t)];
        stroke(&mut img, &frame, 1.0, Rgba([0, 0, 0, 255]));
        let mid = (t + b) / 2.0;
        let amp = |x: f64| 5.0 + 290.0 * ((x - l) / (r - l)).powf(1.6);
        let xs: Vec<f64> = (0..=1600).map(|i| l + (r - l) * i as f64 / 1600.0).collect();
        let curve = |f: &dyn Fn(f64) -> f64| -> Vec<(f64, f64)> { xs.iter().map(|&x| (x, f(x))).collect() };
        let k = std::f64::consts::TAU / 130.0;
        stroke(&mut img, &curve(&|x| mid - amp(x) * (k * (x - l)).sin()), 1.5, Rgba([40, 40, 230, 255]));
        stroke(&mut img, &curve(&|x| mid + amp(x) * (k * (x - l)).sin()), 1.5, Rgba([250, 170, 20, 255]));
        stroke(&mut img, &curve(&|x| mid - amp(x) - 15.0), 1.5, Rgba([30, 150, 30, 255]));
        stroke(&mut img, &curve(&|x| mid + amp(x) + 15.0), 1.5, Rgba([230, 30, 30, 255]));
        img
    }

    #[test]
    fn curve_chords_are_not_perspective() {
        let img = growing_sines_plot();
        let r = detect_correction(&gray(&img));
        assert!(!r.correction.enabled, "{}", r.note);
        assert!(r.rotation_deg.abs() < 0.25, "{}", r.note);
        // Only the frame survives as evidence.
        for (a, b) in r.h_lines.iter().chain(&r.v_lines) {
            let ang = (b.y - a.y).atan2(b.x - a.x).to_degrees().rem_euclid(90.0);
            assert!(ang.min(90.0 - ang) < 0.5, "curve chord kept: {a:?}-{b:?} ({ang:.2}°)");
        }
    }

    #[test]
    fn curve_chords_do_not_spoil_rotation() {
        let img = growing_sines_plot();
        let rot = Homography::rotation_about(3f64.to_radians(), Pt::new(500.0, 375.0));
        let warped = warp_perspective(&img, &rot, 1000, 750);
        let r = detect_correction(&gray(&warped));
        assert_eq!(r.correction.method, "deskew", "{}", r.note);
        assert!((r.rotation_deg - 3.0).abs() < 0.2, "{}", r.note);
    }

    #[test]
    fn straight_image_needs_no_correction() {
        let img = synthetic_plot(600, 400);
        let r = detect_correction(&gray(&img));
        assert!(!r.correction.enabled, "{}", r.note);
    }

    #[test]
    fn recovers_rotation() {
        let img = synthetic_plot(600, 400);
        let rot = Homography::rotation_about(4f64.to_radians(), Pt::new(300.0, 200.0));
        let warped = warp_perspective(&img, &rot, 600, 400);
        let r = detect_correction(&gray(&warped));
        assert!(r.correction.enabled, "{}", r.note);
        assert!((r.rotation_deg - 4.0).abs() < 0.3, "{} {}", r.rotation_deg, r.note);
        // After correction the frame edges are axis-aligned.
        let fixed = warp_perspective(&warped, &r.correction.homography(), r.correction.out_w, r.correction.out_h);
        let r2 = detect_correction(&gray(&fixed));
        assert!(r2.rotation_deg.abs() < 0.3, "{}", r2.note);
    }

    #[test]
    fn recovers_perspective() {
        let img = synthetic_plot(600, 400);
        // Keystone: top edge narrower than the bottom.
        let src = [Pt::new(0.0, 0.0), Pt::new(600.0, 0.0), Pt::new(600.0, 400.0), Pt::new(0.0, 400.0)];
        let dst = [Pt::new(70.0, 30.0), Pt::new(540.0, 10.0), Pt::new(590.0, 390.0), Pt::new(15.0, 370.0)];
        let hm = Homography::fit(&src, &dst).unwrap();
        let warped = warp_perspective(&img, &hm, 600, 400);
        let r = detect_correction(&gray(&warped));
        assert!(r.correction.enabled, "{}", r.note);
        let c = &r.correction;
        let fixed = warp_perspective(&warped, &c.homography(), c.out_w, c.out_h);
        // Frame lines should now be straight: check horizontals/verticals.
        let g = gray(&fixed);
        let ink = detect_ink(&g, 12, 22.0);
        let hl = hough_segments(&ink, 0.0, 6.0, 0.1, 0.3 * g.width() as f64, 4);
        let vl = hough_segments(&ink, 90.0, 6.0, 0.1, 0.3 * g.height() as f64, 4);
        assert!(hl.len() >= 2 && vl.len() >= 2, "{} {}", hl.len(), vl.len());
        for l in hl.iter().take(2) {
            assert!(l.angle_deg().abs() < 0.6, "h {}", l.angle_deg());
        }
        for l in vl.iter().take(2) {
            assert!((l.angle_deg().abs() - 90.0).abs() < 0.6, "v {}", l.angle_deg());
        }
        let _ = Luma([0u8]);
    }
}
