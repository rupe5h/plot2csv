//! 2-D geometry: points, rectangles and homographies.

use nalgebra::{DMatrix, Matrix3, Vector3};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
    pub fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
    pub fn scale(self, s: f64) -> Pt {
        Pt::new(self.x * s, self.y * s)
    }
    pub fn dot(self, o: Pt) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: Pt) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn norm(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn lerp(self, o: Pt, t: f64) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RectF {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl RectF {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
    pub fn area(&self) -> f64 {
        self.width().max(0.0) * self.height().max(0.0)
    }
    pub fn center(&self) -> Pt {
        Pt::new((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }
    pub fn expand(&self, d: f64) -> Self {
        Self { x0: self.x0 - d, y0: self.y0 - d, x1: self.x1 + d, y1: self.y1 + d }
    }
    pub fn intersect(&self, o: &RectF) -> Option<RectF> {
        let r = RectF { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        (r.x1 > r.x0 && r.y1 > r.y0).then_some(r)
    }
    pub fn iou(&self, o: &RectF) -> f64 {
        let i = self.intersect(o).map(|r| r.area()).unwrap_or(0.0);
        let u = self.area() + o.area() - i;
        if u > 0.0 {
            i / u
        } else {
            0.0
        }
    }
    pub fn clamp_to(&self, w: f64, h: f64) -> Self {
        RectF::new(self.x0.clamp(0.0, w), self.y0.clamp(0.0, h), self.x1.clamp(0.0, w), self.y1.clamp(0.0, h))
    }
    pub fn union(&self, o: &RectF) -> RectF {
        RectF { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    /// Corners in the order TL, TR, BR, BL.
    pub fn corners(&self) -> [Pt; 4] {
        [Pt::new(self.x0, self.y0), Pt::new(self.x1, self.y0), Pt::new(self.x1, self.y1), Pt::new(self.x0, self.y1)]
    }
}

/// A projective transform `p' ~ H p`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Homography {
    pub m: [[f64; 3]; 3],
}

impl Default for Homography {
    fn default() -> Self {
        Self::identity()
    }
}

impl Homography {
    pub fn identity() -> Self {
        Self { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] }
    }

    fn mat(&self) -> Matrix3<f64> {
        Matrix3::from_fn(|r, c| self.m[r][c])
    }

    fn from_mat(m: &Matrix3<f64>) -> Self {
        let s = if m[(2, 2)].abs() > 1e-12 { m[(2, 2)] } else { 1.0 };
        let mut out = [[0.0; 3]; 3];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = m[(r, c)] / s;
            }
        }
        Self { m: out }
    }

    pub fn apply(&self, p: Pt) -> Pt {
        let m = &self.m;
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        Pt::new((m[0][0] * p.x + m[0][1] * p.y + m[0][2]) / w, (m[1][0] * p.x + m[1][1] * p.y + m[1][2]) / w)
    }

    /// The homogeneous scale of `p`'s image. Its sign tells which side of the
    /// vanishing line `p` is on: points on opposite sides do not map to one
    /// connected picture.
    pub fn w(&self, p: Pt) -> f64 {
        self.m[2][0] * p.x + self.m[2][1] * p.y + self.m[2][2]
    }

    /// Bounding box of the image of `r` (its four corners).
    pub fn apply_rect(&self, r: &RectF) -> RectF {
        let c = r.corners().map(|p| self.apply(p));
        RectF::new(
            c.iter().map(|p| p.x).fold(f64::MAX, f64::min),
            c.iter().map(|p| p.y).fold(f64::MAX, f64::min),
            c.iter().map(|p| p.x).fold(f64::MIN, f64::max),
            c.iter().map(|p| p.y).fold(f64::MIN, f64::max),
        )
    }

    pub fn inverse(&self) -> Option<Self> {
        self.mat().try_inverse().map(|m| Self::from_mat(&m))
    }

    /// `self ∘ other` (apply `other` first).
    pub fn then(&self, after: &Homography) -> Self {
        Self::from_mat(&(after.mat() * self.mat()))
    }

    pub fn translation(dx: f64, dy: f64) -> Self {
        Self { m: [[1.0, 0.0, dx], [0.0, 1.0, dy], [0.0, 0.0, 1.0]] }
    }

    pub fn rotation_about(angle_rad: f64, c: Pt) -> Self {
        let (s, co) = angle_rad.sin_cos();
        let r = Self { m: [[co, -s, 0.0], [s, co, 0.0], [0.0, 0.0, 1.0]] };
        Self::translation(-c.x, -c.y).then(&r).then(&Self::translation(c.x, c.y))
    }

    /// Least-squares DLT from >= 4 correspondences (with Hartley normalisation).
    pub fn fit(src: &[Pt], dst: &[Pt]) -> Option<Self> {
        let n = src.len();
        if n < 4 || dst.len() != n {
            return None;
        }
        let norm = |pts: &[Pt]| -> Matrix3<f64> {
            let c = pts.iter().fold(Pt::default(), |a, p| a.add(*p)).scale(1.0 / pts.len() as f64);
            let d = pts.iter().map(|p| p.dist(c)).sum::<f64>() / pts.len() as f64;
            let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
            Matrix3::new(s, 0.0, -s * c.x, 0.0, s, -s * c.y, 0.0, 0.0, 1.0)
        };
        let ts = norm(src);
        let td = norm(dst);
        let tp = |t: &Matrix3<f64>, p: Pt| {
            let v = t * Vector3::new(p.x, p.y, 1.0);
            Pt::new(v.x / v.z, v.y / v.z)
        };
        let mut a = DMatrix::<f64>::zeros(2 * n.max(5), 9);
        for i in 0..n {
            let s = tp(&ts, src[i]);
            let d = tp(&td, dst[i]);
            let r = 2 * i;
            let row0 = [-s.x, -s.y, -1.0, 0.0, 0.0, 0.0, d.x * s.x, d.x * s.y, d.x];
            let row1 = [0.0, 0.0, 0.0, -s.x, -s.y, -1.0, d.y * s.x, d.y * s.y, d.y];
            for c in 0..9 {
                a[(r, c)] = row0[c];
                a[(r + 1, c)] = row1[c];
            }
        }
        let svd = a.svd(false, true);
        let vt = svd.v_t?;
        // Smallest singular value -> last row of V^T (nalgebra sorts descending).
        let (mut best, mut best_s) = (0, f64::INFINITY);
        for (i, s) in svd.singular_values.iter().enumerate() {
            if *s < best_s {
                best_s = *s;
                best = i;
            }
        }
        let h = vt.row(best);
        let hn = Matrix3::new(h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]);
        let full = td.try_inverse()? * hn * ts;
        Some(Self::from_mat(&full))
    }
}

/// Intersection of two infinite lines (a0,a1) and (b0,b1).
pub fn line_intersection(a0: Pt, a1: Pt, b0: Pt, b1: Pt) -> Option<Pt> {
    let r = a1.sub(a0);
    let s = b1.sub(b0);
    let d = r.cross(s);
    if d.abs() < 1e-12 {
        return None;
    }
    let t = b0.sub(a0).cross(s) / d;
    Some(a0.add(r.scale(t)))
}

/// Distance from p to the segment ab.
pub fn point_segment_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let ab = b.sub(a);
    let l2 = ab.dot(ab);
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = (p.sub(a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a.add(ab.scale(t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_roundtrip() {
        let src = [Pt::new(10.0, 20.0), Pt::new(400.0, 35.0), Pt::new(390.0, 300.0), Pt::new(25.0, 280.0)];
        let dst = [Pt::new(0.0, 0.0), Pt::new(400.0, 0.0), Pt::new(400.0, 300.0), Pt::new(0.0, 300.0)];
        let h = Homography::fit(&src, &dst).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            let p = h.apply(*s);
            assert!(p.dist(*d) < 1e-6, "{p:?} vs {d:?}");
        }
        let inv = h.inverse().unwrap();
        let q = inv.apply(h.apply(Pt::new(123.0, 77.0)));
        assert!(q.dist(Pt::new(123.0, 77.0)) < 1e-6);
    }

    #[test]
    fn rotation_about_center() {
        let h = Homography::rotation_about(std::f64::consts::FRAC_PI_2, Pt::new(1.0, 1.0));
        let p = h.apply(Pt::new(2.0, 1.0));
        assert!(p.dist(Pt::new(1.0, 2.0)) < 1e-9);
    }

    #[test]
    fn intersect_lines() {
        let p = line_intersection(Pt::new(0.0, 0.0), Pt::new(2.0, 2.0), Pt::new(0.0, 2.0), Pt::new(2.0, 0.0)).unwrap();
        assert!(p.dist(Pt::new(1.0, 1.0)) < 1e-12);
    }
}
