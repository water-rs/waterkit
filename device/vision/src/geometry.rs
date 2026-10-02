//! Geometry primitives shared by the barcode engine and the OCR pipeline.
//!
//! All coordinates are floating-point pixels in the coordinate space of the
//! frame they were measured in (buffer space unless documented otherwise).

#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::suboptimal_flops,
    clippy::needless_range_loop
)]

use std::ops::{Add, Div, Mul, Sub};

/// A 2D point in pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate (column axis).
    pub x: f64,
    /// Vertical coordinate (row axis).
    pub y: f64,
}

impl Point {
    /// Origin point `(0, 0)`.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Create a point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to another point.
    #[must_use]
    pub fn distance(self, other: Self) -> f64 {
        self.delta(other).length()
    }

    /// Vector difference `other - self`.
    #[must_use]
    pub fn delta(self, other: Self) -> Self {
        Self {
            x: other.x - self.x,
            y: other.y - self.y,
        }
    }

    /// Vector length.
    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Cross product `self x other` (z component of the 3D cross product).
    ///
    /// Positive when `other` is clockwise from `self` in a y-down image
    /// coordinate system.
    #[must_use]
    pub fn cross(self, other: Self) -> f64 {
        self.x.mul_add(other.y, -(self.y * other.x))
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x.mul_add(other.x, self.y * other.y)
    }
}

impl Add for Point {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Mul<f64> for Point {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl Div<f64> for Point {
    type Output = Self;
    fn div(self, rhs: f64) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

/// A quadrilateral describing where in the source image a code was found.
///
/// Vertices are ordered clockwise starting at the corner nearest the
/// symbol's logical top-left (in buffer coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quadrilateral {
    /// Top-left vertex.
    pub top_left: Point,
    /// Top-right vertex.
    pub top_right: Point,
    /// Bottom-right vertex.
    pub bottom_right: Point,
    /// Bottom-left vertex.
    pub bottom_left: Point,
}

impl Quadrilateral {
    /// Create a quadrilateral from four vertices in
    /// TL -> TR -> BR -> BL order.
    #[must_use]
    pub const fn new(
        top_left: Point,
        top_right: Point,
        bottom_right: Point,
        bottom_left: Point,
    ) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    /// Vertices as an array in TL -> TR -> BR -> BL order.
    #[must_use]
    pub const fn vertices(&self) -> [Point; 4] {
        [
            self.top_left,
            self.top_right,
            self.bottom_right,
            self.bottom_left,
        ]
    }

    /// Centroid of the four vertices.
    #[must_use]
    pub fn center(&self) -> Point {
        (self.top_left + self.top_right + self.bottom_right + self.bottom_left) / 4.0
    }

    /// Minimum x coordinate of the quadrilateral.
    #[must_use]
    pub const fn min_x(&self) -> f64 {
        self.top_left
            .x
            .min(self.top_right.x)
            .min(self.bottom_right.x)
            .min(self.bottom_left.x)
    }

    /// Minimum y coordinate of the quadrilateral.
    #[must_use]
    pub const fn min_y(&self) -> f64 {
        self.top_left
            .y
            .min(self.top_right.y)
            .min(self.bottom_right.y)
            .min(self.bottom_left.y)
    }

    /// Maximum x coordinate of the quadrilateral.
    #[must_use]
    pub const fn max_x(&self) -> f64 {
        self.top_left
            .x
            .max(self.top_right.x)
            .max(self.bottom_right.x)
            .max(self.bottom_left.x)
    }

    /// Maximum y coordinate of the quadrilateral.
    #[must_use]
    pub const fn max_y(&self) -> f64 {
        self.top_left
            .y
            .max(self.top_right.y)
            .max(self.bottom_right.y)
            .max(self.bottom_left.y)
    }

    /// Axis-aligned bounding width.
    #[must_use]
    pub fn width(&self) -> f64 {
        self.max_x() - self.min_x()
    }

    /// Axis-aligned bounding height.
    #[must_use]
    pub fn height(&self) -> f64 {
        self.max_y() - self.min_y()
    }

    /// Rotate every vertex by `degrees` clockwise within a `width` x
    /// `height` buffer. `degrees` must be a multiple of 90.
    #[must_use]
    pub fn rotated(&self, degrees: i32, width: f64, height: f64) -> Self {
        let rot = |p: Point| -> Point {
            match degrees.rem_euclid(360) {
                90 => Point::new(height - 1.0 - p.y, p.x),
                180 => Point::new(width - 1.0 - p.x, height - 1.0 - p.y),
                270 => Point::new(p.y, width - 1.0 - p.x),
                _ => p,
            }
        };
        Self {
            top_left: rot(self.top_left),
            top_right: rot(self.top_right),
            bottom_right: rot(self.bottom_right),
            bottom_left: rot(self.bottom_left),
        }
    }
}

/// A projective transform (homography) between 2D planes.
///
/// Maps `(u, v)` to `(x, y)` where
/// `x = (a*u + b*v + c) / (g*u + h*v + 1)`,
/// `y = (d*u + e*v + f) / (g*u + h*v + 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
}

impl Homography {
    /// Solve the homography mapping the four `(u, v)` source points onto
    /// the four `(x, y)` destination points, in order.
    ///
    /// Returns `None` when the point set is (numerically) degenerate.
    #[must_use]
    pub fn from_points(src: [Point; 4], dst: [Point; 4]) -> Option<Self> {
        // 8x8 linear system:
        //   x_i*(g*u_i + h*v_i + 1) = a*u_i + b*v_i + c
        //   y_i*(g*u_i + h*v_i + 1) = d*u_i + e*v_i + f
        let mut m = [[0.0f64; 9]; 8];
        for (i, (s, d)) in src.iter().zip(dst.iter()).enumerate() {
            let (u, v, x, y) = (s.x, s.y, d.x, d.y);
            m[2 * i] = [u, v, 1.0, 0.0, 0.0, 0.0, -u * x, -v * x, x];
            m[2 * i + 1] = [0.0, 0.0, 0.0, u, v, 1.0, -u * y, -v * y, y];
        }
        solve_8x8(&mut m).map(|s| Self {
            a: s[0],
            b: s[1],
            c: s[2],
            d: s[3],
            e: s[4],
            f: s[5],
            g: s[6],
            h: s[7],
        })
    }

    /// Solve the affine transform mapping three source points onto three
    /// destination points (projective terms g = h = 0).
    ///
    /// Returns `None` when the source points are collinear.
    #[must_use]
    pub fn affine_from_points(src: [Point; 3], dst: [Point; 3]) -> Option<Self> {
        let (u0, v0) = (src[0].x, src[0].y);
        let (u1, v1) = (src[1].x, src[1].y);
        let (u2, v2) = (src[2].x, src[2].y);
        let det = (u1 - u0).mul_add(v2 - v0, -((u2 - u0) * (v1 - v0)));
        if det.abs() < f64::EPSILON {
            return None;
        }
        // Express (u, v) in the basis (src1 - src0, src2 - src0):
        //   alpha = (t2*(u-u0) - s2*(v-v0)) / det
        //   beta  = (-t1*(u-u0) + s1*(v-v0)) / det
        // then emit dst0 + (dst1-dst0)*alpha + (dst2-dst0)*beta.
        let (s1, s2, t1, t2) = (u1 - u0, u2 - u0, v1 - v0, v2 - v0);
        let d10x = dst[1].x - dst[0].x;
        let d10y = dst[1].y - dst[0].y;
        let d20x = dst[2].x - dst[0].x;
        let d20y = dst[2].y - dst[0].y;
        let a = (d10x * t2 - d20x * t1) / det;
        let b = (-d10x * s2 + d20x * s1) / det;
        let d = (d10y * t2 - d20y * t1) / det;
        let e = (-d10y * s2 + d20y * s1) / det;
        let c = dst[0].x - a * u0 - b * v0;
        let f = dst[0].y - d * u0 - e * v0;
        Some(Self {
            a,
            b,
            c,
            d,
            e,
            f,
            g: 0.0,
            h: 0.0,
        })
    }

    /// Apply the transform to a point.
    ///
    /// Returns `None` when the point lands on the vanishing line
    /// (denominator zero).
    #[must_use]
    pub fn apply(&self, u: f64, v: f64) -> Option<Point> {
        let den = self.g.mul_add(u, self.h.mul_add(v, 1.0));
        if den.abs() < 1e-12 {
            return None;
        }
        Some(Point {
            x: (self.a * u + self.b * v + self.c) / den,
            y: (self.d * u + self.e * v + self.f) / den,
        })
    }
}

/// Gaussian elimination with partial pivoting on an 8x9 augmented matrix.
fn solve_8x8(m: &mut [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let mut pivot = col;
        let mut best = m[col][col].abs();
        for row in (col + 1)..8 {
            let v = m[row][col].abs();
            if v > best {
                best = v;
                pivot = row;
            }
        }
        if best < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        for row in (col + 1)..8 {
            let factor = m[row][col] / m[col][col];
            for k in col..9 {
                m[row][k] -= factor * m[col][k];
            }
        }
    }
    let mut x = [0.0f64; 8];
    for i in (0..8).rev() {
        let mut acc = m[i][8];
        for (k, xk) in x.iter().enumerate().skip(i + 1) {
            acc -= m[i][k] * xk;
        }
        x[i] = acc / m[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_identity() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(0.0, 1.0),
            Point::new(1.0, 1.0),
        ];
        let h = Homography::from_points(pts, pts).unwrap();
        let p = h.apply(0.3, 0.7).unwrap();
        assert!((p.x - 0.3).abs() < 1e-9 && (p.y - 0.7).abs() < 1e-9);
    }

    #[test]
    fn homography_rotation_90() {
        // Grid (0..10)^2 rotated 90° CCW-ish: (u,v) -> (v, 10-u)
        let src = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0),
            Point::new(10.0, 10.0),
        ];
        let dst = [
            Point::new(0.0, 10.0),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(10.0, 0.0),
        ];
        let h = Homography::from_points(src, dst).unwrap();
        for (u, v) in [(3.0, 4.0), (7.5, 2.5), (10.0, 10.0)] {
            let p = h.apply(u, v).unwrap();
            assert!(
                (p.x - v).abs() < 1e-9 && (p.y - (10.0 - u)).abs() < 1e-9,
                "got {p:?} for ({u},{v})"
            );
        }
    }

    #[test]
    fn affine_roundtrip() {
        let src = [
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(0.0, 2.0),
        ];
        let dst = [
            Point::new(1.0, 1.0),
            Point::new(9.0, 3.0),
            Point::new(-1.0, 5.0),
        ];
        let h = Homography::affine_from_points(src, dst).unwrap();
        for s in src {
            let p = h.apply(s.x, s.y).unwrap();
            assert!(p.distance(h.apply(s.x, s.y).unwrap()) < 1e-9);
        }
        let p = h.apply(0.0, 0.0).unwrap();
        assert!((p.x - 1.0).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9);
        let p = h.apply(4.0, 0.0).unwrap();
        assert!((p.x - 9.0).abs() < 1e-9 && (p.y - 3.0).abs() < 1e-9);
    }
}
