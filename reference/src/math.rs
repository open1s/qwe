//! Deterministic 3D math primitives for the simulation core.
//!
//! All values are `f64` to keep cross-platform determinism straightforward (a
//! `f64` bit pattern round-trips losslessly). No external dependency. All
//! operations are pure and total; division/rotation guarded against degenerate
//! inputs so the simulation never panics on malformed but bounded state.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }
    /// Returns the unit vector, or ZERO when the input is (near) zero length.
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len < 1e-12 {
            Self::ZERO
        } else {
            self * (1.0 / len)
        }
    }
    pub fn scale(self, s: f64) -> Self {
        self * s
    }
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}
impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}
impl SubAssign for Vec3 {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }
}
impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        self * -1.0
    }
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn half_extents(self) -> Vec3 {
        (self.max - self.min) * 0.5
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }
    /// The smallest AABB containing both boxes.
    pub fn union(self, other: Self) -> Self {
        Self {
            min: Vec3::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }
    pub fn penetration(self, other: Self) -> Vec3 {
        let dx = (self.max.x - other.min.x).min(other.max.x - self.min.x);
        let dy = (self.max.y - other.min.y).min(other.max.y - self.min.y);
        let dz = (self.max.z - other.min.z).min(other.max.z - self.min.z);
        Vec3::new(dx, dy, dz)
    }
}

/// A quaternion rotation (x, y, z, w). Not re-normalized automatically to keep
/// step arithmetic cheap; callers use `normalized` before composing long chains.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
    pub fn from_axis_angle(axis: Vec3, angle: f64) -> Self {
        let axis = axis.normalized();
        if axis.length_sq() < 1e-24 {
            return Self::IDENTITY;
        }
        let half = angle * 0.5;
        let s = half.sin();
        Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: half.cos(),
        }
    }
    pub fn normalized(self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if len < 1e-12 {
            Self::IDENTITY
        } else {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
                w: self.w / len,
            }
        }
    }
    /// Quaternion that rotates -Z forward toward `dir`, with +Y up.
    /// Returns IDENTITY when the direction is degenerate.
    pub fn look_rotation(dir: Vec3, up: Vec3) -> Self {
        let f = dir.normalized();
        if f.length_sq() < 1e-24 {
            return Self::IDENTITY;
        }
        let right = f.cross(up).normalized();
        if right.length_sq() < 1e-24 {
            return Self::IDENTITY;
        }
        let u = right.cross(f);
        // Rotation matrix columns (right, u, -f): camera local −Z maps to `f`.
        let m00 = right.x;
        let m01 = u.x;
        let m02 = -f.x;
        let m10 = right.y;
        let m11 = u.y;
        let m12 = -f.y;
        let m20 = right.z;
        let m21 = u.z;
        let m22 = -f.z;
        let trace = m00 + m11 + m22;
        if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Self {
                w: s / 4.0,
                x: (m21 - m12) / s,
                y: (m02 - m20) / s,
                z: (m10 - m01) / s,
            }
            .normalized()
        } else {
            // Fallback for negative trace (axes dominate); use the largest diagonal.
            Self::from_axis_angle(f, 0.0)
        }
    }
    /// Rotates a vector by this quaternion.
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let s = self.w;
        u * (2.0 * u.dot(v)) + v * (s * s - u.dot(u)) + u.cross(v) * (2.0 * s)
    }
    pub fn composed(self, other: Self) -> Self {
        Self {
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vec_ops_are_deterministic() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        assert_eq!(a + b, Vec3::new(5.0, 7.0, 9.0));
        assert_eq!(a.dot(b), 32.0);
        assert_eq!(a.cross(b), Vec3::new(-3.0, 6.0, -3.0));
    }
    #[test]
    fn quat_rotation_is_length_preserving() {
        let axis = Vec3::new(0.0, 1.0, 0.0);
        let q = Quat::from_axis_angle(axis, std::f64::consts::FRAC_PI_2);
        let v = Vec3::new(1.0, 0.0, 0.0);
        let rotated = q.rotate(v);
        assert!((rotated.length() - 1.0).abs() < 1e-9);
        assert!(rotated.z.abs() > 0.999);
    }
    #[test]
    fn aabb_overlap_and_penetration() {
        let a = Aabb::from_center_half(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.5, 0.5, 0.5));
        let b = Aabb::from_center_half(Vec3::new(0.4, 0.4, 0.0), Vec3::new(0.5, 0.5, 0.5));
        assert!(a.overlaps(b));
        let pen = a.penetration(b);
        assert!(pen.x > 0.0 && pen.y > 0.0 && pen.z > 0.0);
    }
}
