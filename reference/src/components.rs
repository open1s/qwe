//! Typed simulation components with canonical byte encoding.
//!
//! The authoritative World stores component values as canonical bytes (RFC-0031
//! "ABI bytes are canonical schema values"). These typed views encode/decode
//! those bytes so a simulation can read and write them without ever diverging
//! from the World as the single source of truth.

use crate::math::{Quat, Vec3};
use pwe_api::{Error, Result, Status};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

// --- canonical byte helpers (deterministic f64) ------------------------------

fn push_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}
fn push_vec3(out: &mut Vec<u8>, v: Vec3) {
    push_f64(out, v.x);
    push_f64(out, v.y);
    push_f64(out, v.z);
}
fn push_quat(out: &mut Vec<u8>, q: Quat) {
    push_f64(out, q.x);
    push_f64(out, q.y);
    push_f64(out, q.z);
    push_f64(out, q.w);
}

fn take_f64(bytes: &[u8], at: usize) -> Result<f64> {
    let slice = bytes.get(at..at + 8).ok_or(error(Status::Invalid, 1))?;
    Ok(f64::from_bits(u64::from_le_bytes(
        slice.try_into().unwrap(),
    )))
}
fn take_vec3(bytes: &[u8], at: usize) -> Result<Vec3> {
    Ok(Vec3::new(
        take_f64(bytes, at)?,
        take_f64(bytes, at + 8)?,
        take_f64(bytes, at + 16)?,
    ))
}
fn take_quat(bytes: &[u8], at: usize) -> Result<Quat> {
    Ok(Quat {
        x: take_f64(bytes, at)?,
        y: take_f64(bytes, at + 8)?,
        z: take_f64(bytes, at + 16)?,
        w: take_f64(bytes, at + 24)?,
    })
}

// --- component types ---------------------------------------------------------

/// World-space position and orientation. 56 bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}
impl Transform {
    pub const BYTES: usize = 7 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        push_vec3(&mut out, self.position);
        push_quat(&mut out, self.rotation);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            position: take_vec3(bytes, 0)?,
            rotation: take_quat(bytes, 24)?,
        })
    }
}

/// Linear and angular velocity. 48 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Velocity {
    pub linear: Vec3,
    pub angular: Vec3,
}
impl Velocity {
    pub const BYTES: usize = 6 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        push_vec3(&mut out, self.linear);
        push_vec3(&mut out, self.angular);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            linear: take_vec3(bytes, 0)?,
            angular: take_vec3(bytes, 24)?,
        })
    }
}

/// Accumulated force (and torque as a Vec3) for one step; reset each step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Force {
    pub force: Vec3,
    pub torque: Vec3,
}
impl Force {
    pub const BYTES: usize = 6 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        push_vec3(&mut out, self.force);
        push_vec3(&mut out, self.torque);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            force: take_vec3(bytes, 0)?,
            torque: take_vec3(bytes, 24)?,
        })
    }
}

/// Rigid-body material/dynamics parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidBody {
    pub mass: f64,
    /// Restitution (bounce), clamped to [0,1] on encode.
    pub restitution: f64,
    /// Coulomb friction coefficient.
    pub friction: f64,
    /// `1` for dynamic (moves), `0` for kinematic/static.
    pub is_dynamic: u8,
}
impl RigidBody {
    pub const BYTES: usize = 3 * 8 + 1;
    pub fn dynamic(mass: f64) -> Self {
        Self {
            mass,
            restitution: 0.3,
            friction: 0.5,
            is_dynamic: 1,
        }
    }
    pub fn r#static() -> Self {
        Self {
            mass: f64::INFINITY,
            restitution: 0.0,
            friction: 0.8,
            is_dynamic: 0,
        }
    }
    pub fn inverse_mass(self) -> f64 {
        if self.is_dynamic == 0 || self.mass <= 0.0 {
            0.0
        } else {
            1.0 / self.mass
        }
    }
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        push_f64(&mut out, self.mass);
        push_f64(&mut out, self.restitution.clamp(0.0, 1.0));
        push_f64(&mut out, self.friction);
        out.push(self.is_dynamic);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            mass: take_f64(bytes, 0)?,
            restitution: take_f64(bytes, 8)?,
            friction: take_f64(bytes, 16)?,
            is_dynamic: *bytes.get(24).ok_or(error(Status::Invalid, 2))?,
        })
    }
}

/// A generic scalar state vector (`N` f64 slots) on an entity. This lets a
/// domain system evolve arbitrary state variables (position, temperature,
/// population, charge, reactant concentration, …) — the carrier for
/// user-defined dynamical systems in the language. Fixed `MAX_STATE_SLOTS`
/// for a bounded, canonical ABI.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub values: Vec<f64>,
}
impl State {
    pub const MAX_STATE_SLOTS: usize = 16;
    pub const SLOT_BYTES: usize = 8;

    pub fn new(values: Vec<f64>) -> Self {
        let mut v = values;
        v.resize(Self::MAX_STATE_SLOTS, 0.0);
        Self { values: v }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::MAX_STATE_SLOTS * Self::SLOT_BYTES);
        for i in 0..Self::MAX_STATE_SLOTS {
            push_f64(&mut out, self.values[i]);
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut values = Vec::with_capacity(Self::MAX_STATE_SLOTS);
        for i in 0..Self::MAX_STATE_SLOTS {
            values.push(take_f64(bytes, i * Self::SLOT_BYTES)?);
        }
        Ok(Self { values })
    }
}

/// A 2.5D terrain heightfield: a grid of sampled `height values, each row
/// contestfulness one world step of `col_step` in the x direction and each
/// column one step of `row_step` in the z direction. The surface height at a
/// (row, col) grid sample is `heights[row * cols + col]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Heightfield {
    pub rows: u16,
    pub cols: u16,
    /// World-space distance between adjacent rows (z axis).
    pub row_step: f64,
    /// World-space distance between adjacent columns (x axis).
    pub col_step: f64,
    /// World-space corner of sample (0,0). Applied as a translation (y0..height).
    pub origin: Vec3,
    /// Row-major height samples. Heights are world-space y values.
    pub heights: Vec<f64>,
}

impl Heightfield {
    pub fn new(rows: u16, cols: u16, row_step: f64, col_step: f64, origin: Vec3) -> Self {
        Self {
            rows,
            cols,
            row_step,
            col_step,
            origin,
            heights: vec![0.0f64; rows as usize * cols as usize],
        }
    }
    /// Height at world (x, z), via nearest-neighbor sampling of the grid.
    pub fn height_at(&self, x: f64, z: f64) -> f64 {
        let rx = ((x - self.origin.x) / self.col_step)
            .round()
            .clamp(0.0, self.cols as f64 - 1.0);
        let rz = ((z - self.origin.z) / self.row_step)
            .round()
            .clamp(0.0, self.rows as f64 - 1.0);
        let row = rz as usize;
        let col = rx as usize;
        self.heights[row * self.cols as usize + col]
    }
    /// Smallest axis-aligned bounding box of the terrain (used by broad phase).
    pub fn world_aabb(&self) -> crate::math::Aabb {
        let width = (self.cols - 1) as f64 * self.col_step;
        let depth = (self.rows - 1) as f64 * self.row_step;
        let min_h = self.heights.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_h = self
            .heights
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        crate::math::Aabb {
            min: Vec3::new(self.origin.x, min_h, self.origin.z),
            max: Vec3::new(self.origin.x + width, max_h, self.origin.z + depth),
        }
    }
}

/// A collision shape. Box and Sphere are the primitive case. Compound is a list
/// of primitive shapes (e.g. a vehicle chassis + wheel spheres).
#[derive(Clone, Debug, PartialEq)]
pub enum Collider {
    /// Axis-aligned box with center at `offset` and half-extents `dims`.
    Box { dims: Vec3, offset: Vec3 },
    /// Sphere with center at `offset` and radius `radius`.
    Sphere { radius: f64, offset: Vec3 },
    /// Any number of child shapes; all share the same transform and dynamics.
    Compound(Vec<Collider>),
    /// Terrain heightfield (see `Heightfield`). A heightfield is immovable and
    /// always considered static.
    Heightfield(Heightfield),
    /// A convex polytope defined by its vertices (world space at the collider's
    /// transform). Narrow phase uses deterministic GJK + EPA.
    ConvexHull { points: Vec<Vec3> },
}

impl Collider {
    /// Back-compatible shorthand for a box.
    pub fn aabb(dims: Vec3) -> Self {
        Self::Box {
            dims,
            offset: Vec3::ZERO,
        }
    }
    /// Back-compatible shorthand for a sphere.
    pub fn sphere(radius: f64) -> Self {
        Self::Sphere {
            radius,
            offset: Vec3::ZERO,
        }
    }
    /// A compound of spheres + boxes (e.g. a vehicle with chassis and wheels).
    pub fn compound(parts: Vec<Collider>) -> Self {
        Self::Compound(parts)
    }
    /// A terrain heightfield.
    pub fn heightfield(field: Heightfield) -> Self {
        Self::Heightfield(field)
    }
    /// A convex hull over `points`.
    pub fn convex_hull(points: Vec<Vec3>) -> Self {
        Self::ConvexHull { points }
    }

    /// World-space bounding box of this shape at `transform`. Compound is the
    /// union of its children; heightfield is its own AABB (offset ignored).
    pub fn world_aabb(&self, position: Vec3) -> crate::math::Aabb {
        match self {
            Collider::Box { dims, offset } => {
                crate::math::Aabb::from_center_half(position + *offset, *dims)
            }
            Collider::Sphere { radius, offset } => crate::math::Aabb::from_center_half(
                position + *offset,
                Vec3::new(*radius, *radius, *radius),
            ),
            Collider::Compound(sub) => {
                if sub.is_empty() {
                    return crate::math::Aabb::from_center_half(position, Vec3::ZERO);
                }
                let mut out = sub[0].world_aabb(position);
                for s in &sub[1..] {
                    let o = s.world_aabb(position);
                    out.min.x = out.min.x.min(o.min.x);
                    out.min.y = out.min.y.min(o.min.y);
                    out.min.z = out.min.z.min(o.min.z);
                    out.max.x = out.max.x.max(o.max.x);
                    out.max.y = out.max.y.max(o.max.y);
                    out.max.z = out.max.z.max(o.max.z);
                }
                out
            }
            Collider::Heightfield(hf) => hf.world_aabb(),
            Collider::ConvexHull { points } => {
                let mut lo = points[0];
                let mut hi = points[0];
                for p in &points[1..] {
                    lo.x = lo.x.min(p.x);
                    lo.y = lo.y.min(p.y);
                    lo.z = lo.z.min(p.z);
                    hi.x = hi.x.max(p.x);
                    hi.y = hi.y.max(p.y);
                    hi.z = hi.z.max(p.z);
                }
                crate::math::Aabb {
                    min: position + lo,
                    max: position + hi,
                }
            }
        }
    }

    /// Canonical byte encoding (kind-prefixed recursive encoding, bounded by
    /// RFC-0034's nesting cap).
    pub fn encode(&self) -> Vec<u8> {
        self.encode_depth(0)
    }
    fn encode_depth(&self, depth: u8) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Collider::Box { dims, offset } => {
                out.push(0u8);
                push_vec3(&mut out, *dims);
                push_vec3(&mut out, *offset);
            }
            Collider::Sphere { radius, offset } => {
                out.push(1u8);
                push_f64(&mut out, *radius);
                push_vec3(&mut out, *offset);
            }
            Collider::Compound(parts) => {
                out.push(3u8);
                out.push((parts.len() as u16).to_le_bytes()[0]);
                out.push((parts.len() as u16).to_le_bytes()[1]);
                // Each child prefixed by u32 byte length.
                if depth < 64 {
                    for p in parts {
                        let child = p.encode_depth(depth + 1);
                        out.extend_from_slice(&(child.len() as u32).to_le_bytes());
                        out.extend_from_slice(&child);
                    }
                }
            }
            Collider::Heightfield(hf) => {
                out.push(4u8);
                out.extend_from_slice(&hf.rows.to_le_bytes());
                out.extend_from_slice(&hf.cols.to_le_bytes());
                push_f64(&mut out, hf.row_step);
                push_f64(&mut out, hf.col_step);
                push_vec3(&mut out, hf.origin);
                out.extend_from_slice(&(hf.heights.len() as u32).to_le_bytes());
                for h in &hf.heights {
                    push_f64(&mut out, *h);
                }
            }
            Collider::ConvexHull { points } => {
                out.push(5u8);
                out.extend_from_slice(&(points.len() as u32).to_le_bytes());
                for p in points {
                    push_vec3(&mut out, *p);
                }
            }
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Self::decode_depth(bytes, 0)
    }
    fn decode_depth(bytes: &[u8], depth: u8) -> Result<Self> {
        if depth > 64 {
            return Err(error(Status::Limit, 1));
        }
        let kind = *bytes.first().ok_or(error(Status::Invalid, 6))?;
        match kind {
            0 => Ok(Collider::Box {
                dims: take_vec3(bytes, 1)?,
                offset: take_vec3(bytes, 25)?,
            }),
            1 => Ok(Collider::Sphere {
                radius: take_f64(bytes, 1)?,
                offset: take_vec3(bytes, 9)?,
            }),
            3 => {
                let count_raw = bytes.get(1..3).ok_or(error(Status::Invalid, 7))?;
                let parts_len = u16::from_le_bytes(count_raw.try_into().unwrap()) as usize;
                let mut cursor = 3;
                let mut parts = Vec::with_capacity(parts_len);
                for _ in 0..parts_len {
                    let len_bytes = bytes
                        .get(cursor..cursor + 4)
                        .ok_or(error(Status::Invalid, 8))?;
                    let len = u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
                    cursor += 4;
                    let slice = bytes
                        .get(cursor..cursor + len)
                        .ok_or(error(Status::Invalid, 9))?;
                    parts.push(Collider::decode_depth(slice, depth + 1)?);
                    cursor += len;
                }
                Ok(Collider::Compound(parts))
            }
            4 => {
                let rows_bytes = bytes.get(1..3).ok_or(error(Status::Invalid, 10))?;
                let rows = u16::from_le_bytes(rows_bytes.try_into().unwrap());
                let cols_bytes = bytes.get(3..5).ok_or(error(Status::Invalid, 11))?;
                let cols = u16::from_le_bytes(cols_bytes.try_into().unwrap());
                let row_step = take_f64(bytes, 5)?;
                let col_step = take_f64(bytes, 13)?;
                let origin = take_vec3(bytes, 21)?;
                let count = u32::from_le_bytes(
                    bytes
                        .get(45..49)
                        .ok_or(error(Status::Invalid, 12))?
                        .try_into()
                        .unwrap(),
                ) as usize;
                let mut heights = Vec::with_capacity(count);
                let mut cursor = 49;
                for _ in 0..count {
                    heights.push(take_f64(bytes, cursor)?);
                    cursor += 8;
                }
                Ok(Collider::Heightfield(Heightfield {
                    rows,
                    cols,
                    row_step,
                    col_step,
                    origin,
                    heights,
                }))
            }
            5 => {
                let count = u32::from_le_bytes(
                    bytes
                        .get(1..5)
                        .ok_or(error(Status::Invalid, 14))?
                        .try_into()
                        .unwrap(),
                ) as usize;
                let mut points = Vec::with_capacity(count);
                let mut cursor = 5;
                for _ in 0..count {
                    points.push(take_vec3(bytes, cursor)?);
                    cursor += 24;
                }
                Ok(Collider::ConvexHull { points })
            }
            _ => Err(error(Status::Invalid, 13)),
        }
    }
}

/// A frustum camera whose pose is driven by a tracked/kinematic transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub fov_y_radians: f64,
    pub near: f64,
    pub far: f64,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_radians: std::f64::consts::FRAC_PI_3, // 60 degrees
            near: 0.1,
            far: 1000.0,
        }
    }
}
impl Camera {
    pub const BYTES: usize = 3 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        push_f64(&mut out, self.fov_y_radians);
        push_f64(&mut out, self.near);
        push_f64(&mut out, self.far);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            fov_y_radians: take_f64(bytes, 0)?,
            near: take_f64(bytes, 8)?,
            far: take_f64(bytes, 16)?,
        })
    }
}

/// A hard distance constraint between two entities. Solved by position
/// relaxation in the physics step (RFC-0008 constraint stage).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistanceJoint {
    pub other: pwe_api::EntityId,
    pub rest_length: f64,
    /// 0..1 relaxation strength per solver iteration.
    pub stiffness: f64,
}
impl DistanceJoint {
    pub const BYTES: usize = 16 + 2 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        out.extend_from_slice(&self.other.0.to_le_bytes());
        push_f64(&mut out, self.rest_length);
        push_f64(&mut out, self.stiffness);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let other = bytes.get(0..16).ok_or(error(Status::Invalid, 4))?;
        Ok(Self {
            other: pwe_api::EntityId(u128::from_le_bytes(other.try_into().unwrap())),
            rest_length: take_f64(bytes, 16)?,
            stiffness: take_f64(bytes, 24)?,
        })
    }
}

/// A joint connecting two entities. Distance keeps a fixed gap; Hinge welds an
/// anchor point and allows swing about `axis`; Prismatic allows translation
/// along `axis` only (perpendicular motion is removed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Joint {
    Distance(DistanceJoint),
    Hinge(HingeJoint),
    Prismatic(PrismaticJoint),
}

impl Joint {
    /// The other entity this joint acts on.
    pub fn other(self) -> pwe_api::EntityId {
        match self {
            Joint::Distance(j) => j.other,
            Joint::Hinge(j) => j.other,
            Joint::Prismatic(j) => j.other,
        }
    }
}

/// Hinge: the `anchor` point (world offset of each body's transform) is welded
/// between the two bodies; both may rotate around `axis`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HingeJoint {
    pub other: pwe_api::EntityId,
    pub axis: Vec3,
    pub anchor: Vec3,
}

/// Prismatic: relative motion is constrained to the shared `axis`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrismaticJoint {
    pub other: pwe_api::EntityId,
    pub axis: Vec3,
}

impl Joint {
    pub const BYTES: usize = 1 + 16 + 8 + 6 * 8;
    pub fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        match self {
            Joint::Distance(j) => {
                out.push(0);
                out.extend_from_slice(&j.other.0.to_le_bytes());
                push_f64(&mut out, j.rest_length);
                push_f64(&mut out, j.stiffness);
                push_vec3(&mut out, Vec3::ZERO);
                push_vec3(&mut out, Vec3::ZERO);
            }
            Joint::Hinge(j) => {
                out.push(1);
                out.extend_from_slice(&j.other.0.to_le_bytes());
                push_f64(&mut out, 0.0);
                push_f64(&mut out, 0.0);
                push_vec3(&mut out, j.axis);
                push_vec3(&mut out, j.anchor);
            }
            Joint::Prismatic(j) => {
                out.push(2);
                out.extend_from_slice(&j.other.0.to_le_bytes());
                push_f64(&mut out, 0.0);
                push_f64(&mut out, 0.0);
                push_vec3(&mut out, j.axis);
                push_vec3(&mut out, Vec3::ZERO);
            }
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let kind = *bytes.first().ok_or(error(Status::Invalid, 6))?;
        let other_raw = bytes.get(1..17).ok_or(error(Status::Invalid, 7))?;
        let other = pwe_api::EntityId(u128::from_le_bytes(other_raw.try_into().unwrap()));
        let rest = take_f64(bytes, 17)?;
        let stiffness = take_f64(bytes, 25)?;
        let v1 = take_vec3(bytes, 33)?;
        let v2 = take_vec3(bytes, 57)?;
        Ok(match kind {
            0 => Self::Distance(DistanceJoint {
                other,
                rest_length: rest,
                stiffness,
            }),
            1 => Self::Hinge(HingeJoint {
                other,
                axis: v1,
                anchor: v2,
            }),
            2 => Self::Prismatic(PrismaticJoint { other, axis: v1 }),
            _ => return Err(error(Status::Invalid, 8)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transform_round_trips_bytes() {
        let t = Transform {
            position: Vec3::new(1.0, -2.5, 3.0),
            rotation: Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 0.37),
        };
        assert_eq!(Transform::decode(&t.encode()).unwrap(), t);
    }
    #[test]
    fn rigid_body_clamps_restitution_on_encode() {
        let rb = RigidBody {
            mass: 2.0,
            restitution: 5.0,
            friction: 0.1,
            is_dynamic: 1,
        };
        let decoded = RigidBody::decode(&rb.encode()).unwrap();
        assert_eq!(decoded.restitution, 1.0);
    }
    #[test]
    fn collider_world_aabb_offsets_center() {
        let c = Collider::aabb(Vec3::new(1.0, 1.0, 1.0));
        let t = Transform::default();
        let aabb = c.world_aabb(t.position);
        assert_eq!(aabb.min, Vec3::new(-1.0, -1.0, -1.0));
        assert_eq!(aabb.max, Vec3::new(1.0, 1.0, 1.0));
    }
    #[test]
    fn convex_hull_encodes_and_decodes_canonically() {
        let hull = Collider::convex_hull(vec![
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(-0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
        ]);
        let decoded = Collider::decode(&hull.encode()).unwrap();
        assert_eq!(decoded, hull);
    }
}

/// Presentation-only style overrides for how an entity is drawn by the viewer.
/// Set from the language's per-entity render attributes (`shape`, `size`,
/// `opacity`, `glow`, `label`); it never affects simulation semantics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderStyle {
    /// `0` = point marker, `1` = sphere, `2` = box (overrides the collider).
    pub shape: Option<u8>,
    /// Marker size / sphere radius / box edge length (world units).
    pub size: Option<f64>,
    /// Per-axis box dimensions (`size = (dx, dy, dz)`), for long thin links.
    pub size3: Option<(f64, f64, f64)>,
    /// Material opacity in `[0, 1]`.
    pub opacity: Option<f64>,
    /// Emissive glow intensity (0 = matte, >0 = self-lit).
    pub glow: Option<f64>,
    /// Whether to draw the floating name label (default `true`).
    pub label: Option<bool>,
}
