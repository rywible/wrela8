//! Pinhole camera and the ray-bundle wedge that affine arithmetic runs over.
//!
//! plans/graphics.md §2.1 classifies *screen tiles*, not world boxes, so the
//! enclosure domain is a frustum wedge: a rectangle of screen directions
//! crossed with a range of `t`. The three affine noise symbols are exactly
//! that — ε₀ = u, ε₁ = v, ε₂ = t.
//!
//! The direction is normalised **inside** affine arithmetic rather than
//! bounded by its four corner rays. Corner bounds would be cheaper and
//! subtly unsound: `normalize` is nonlinear, so the set of unit directions
//! over a tile is not the bilinear hull of the corner unit directions, and a
//! probe that quietly widened by "a small safety factor" would be reporting
//! its own fudge as an interior-tile fraction.

use crate::aff::Aff;
use crate::eval::DAff;

#[derive(Clone, Copy)]
pub struct Camera {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub fwd: [f32; 3],
    pub tan_half: f32,
    pub aspect: f32,
    pub w: u32,
    pub h: u32,
    pub fov_deg: f32,
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

impl Camera {
    pub fn look_at(
        eye: [f32; 3],
        target: [f32; 3],
        fov_deg: f32,
        w: u32,
        h: u32,
    ) -> Camera {
        let fwd = norm3([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
        let right = norm3(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        Camera {
            eye,
            right,
            up,
            fwd,
            tan_half: (fov_deg.to_radians() * 0.5).tan(),
            aspect: w as f32 / h as f32,
            w,
            h,
            fov_deg,
        }
    }

    /// Screen-x in pixels (continuous, 0..w) to the `u` coefficient.
    #[inline]
    pub fn u_of(&self, px: f32) -> f32 {
        (2.0 * px / self.w as f32 - 1.0) * self.aspect * self.tan_half
    }

    /// Screen-y in pixels (continuous, 0..h) to the `v` coefficient.
    #[inline]
    pub fn v_of(&self, py: f32) -> f32 {
        (1.0 - 2.0 * py / self.h as f32) * self.tan_half
    }

    #[inline]
    pub fn dir(&self, u: f32, v: f32) -> [f32; 3] {
        norm3([
            self.fwd[0] + u * self.right[0] + v * self.up[0],
            self.fwd[1] + u * self.right[1] + v * self.up[1],
            self.fwd[2] + u * self.right[2] + v * self.up[2],
        ])
    }

    #[inline]
    pub fn dir_at_pixel(&self, px: f32, py: f32) -> [f32; 3] {
        self.dir(self.u_of(px), self.v_of(py))
    }

    /// Affine enclosure of the unit direction over a `(u, v)` rectangle.
    fn dir_aff(&self, u0: f32, u1: f32, v0: f32, v1: f32) -> [Aff; 3] {
        let u = Aff::sym(0, u0.min(u1), u0.max(u1));
        let v = Aff::sym(1, v0.min(v1), v0.max(v1));
        let mut d = [Aff::konst(0.0); 3];
        for i in 0..3 {
            d[i] = Aff::konst(self.fwd[i]).add(u.mul_c(self.right[i])).add(v.mul_c(self.up[i]));
        }
        let l2 = d[0].square().add(d[1].square()).add(d[2].square());
        let inv = l2.sqrt().recip();
        [d[0].mul(inv), d[1].mul(inv), d[2].mul(inv)]
    }

    /// Position enclosure over the wedge `(u,v) × [t0,t1]`.
    pub fn wedge(&self, u0: f32, u1: f32, v0: f32, v1: f32, t0: f32, t1: f32) -> [Aff; 3] {
        let dh = self.dir_aff(u0, u1, v0, v1);
        let t = Aff::sym(2, t0, t1);
        [
            dh[0].mul(t).add_c(self.eye[0]),
            dh[1].mul(t).add_c(self.eye[1]),
            dh[2].mul(t).add_c(self.eye[2]),
        ]
    }

    /// The same wedge, carrying `∂p/∂t` — which is the unit direction
    /// exactly. This is what makes the silhouette certificate cheap: the
    /// directional derivative `∂f/∂t = ∇f·d̂` falls out of one dual-affine
    /// pass, with no separate gradient program.
    pub fn wedge_daff(
        &self,
        u0: f32,
        u1: f32,
        v0: f32,
        v1: f32,
        t0: f32,
        t1: f32,
    ) -> [DAff; 3] {
        let dh = self.dir_aff(u0, u1, v0, v1);
        let t = Aff::sym(2, t0, t1);
        [
            DAff { v: dh[0].mul(t).add_c(self.eye[0]), dt: dh[0] },
            DAff { v: dh[1].mul(t).add_c(self.eye[1]), dt: dh[1] },
            DAff { v: dh[2].mul(t).add_c(self.eye[2]), dt: dh[2] },
        ]
    }

    /// Wedge at a fixed `t` — a screen rectangle at one depth. Used to prove
    /// "exactly one crossing" by showing the field is positive across the
    /// whole near face and negative across the whole far face.
    pub fn slice(&self, u0: f32, u1: f32, v0: f32, v1: f32, t: f32) -> [Aff; 3] {
        let dh = self.dir_aff(u0, u1, v0, v1);
        [
            dh[0].mul_c(t).add_c(self.eye[0]),
            dh[1].mul_c(t).add_c(self.eye[1]),
            dh[2].mul_c(t).add_c(self.eye[2]),
        ]
    }
}
