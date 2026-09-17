//! Deterministic fixed-step rigid-body dynamics.
//!
//! A single `step(dt)` performs: integrate (gravity + forces + damping),
//! integrate velocity into position, broad-phase AABB candidate generation,
//! narrow-phase overlap resolution with positional correction and impulses, and
//! a ground-plane contact constraint at `y = 0`. Entities are processed in
//! stable id order so the result is bit-for-bit reproducible.

use crate::components::{Collider, Heightfield, Transform, Velocity};
use crate::math::{Aabb, Vec3};
use crate::scene::Scene;
use pwe_api::EntityId;

/// A resolved contact point between two entities (or one entity and ground).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub a: u128,
    pub b: u128, // u128::MAX encodes the static ground plane
    pub normal: Vec3,
    pub penetration: f64,
    pub point: Vec3,
}

pub const GROUND: u128 = u128::MAX;

/// The interchangeable broad-phase backend the physics system uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadPhaseKind {
    /// Uniform grid (conservative candidate set).
    Grid,
    /// Median-split BVH (exact candidate set).
    Bvh,
}

pub struct PhysicsConfig {
    /// Linear damping applied each step (0 = none, scales velocity).
    pub linear_damping: f64,
    /// Angular damping factor.
    pub angular_damping: f64,
    /// Solver iterations for contact resolution.
    pub solver_iterations: u32,
    /// Maximum correction per step as a fraction of penetration.
    pub correction_factor: f64,
    /// Restitution fallback when a body does not declare one.
    pub default_restitution: f64,
    /// Friction fallback when a body does not declare one.
    pub default_friction: f64,
    /// Broad-phase backend (RFC-0008 §7 interchangeable backends).
    pub broad_phase: BroadPhaseKind,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            linear_damping: 0.995,
            angular_damping: 0.995,
            solver_iterations: 4,
            correction_factor: 0.8,
            default_restitution: 0.3,
            default_friction: 0.5,
            broad_phase: BroadPhaseKind::Grid,
        }
    }
}

pub struct PhysicsSystem {
    pub config: PhysicsConfig,
}

impl PhysicsSystem {
    pub fn new(config: PhysicsConfig) -> Self {
        Self { config }
    }

    /// Advances the scene by one fixed `dt` step and returns the contacts that
    /// resolved this step (deterministic ordering).
    pub fn step(&self, scene: &mut Scene, dt: f64, contacts: &mut Vec<Contact>) {
        contacts.clear();

        // 1. Integrate forces and velocities.
        let gravity = scene.gravity;
        for (_id, e) in scene.iter_mut() {
            let is_dynamic = e.rigid_body.map(|rb| rb.is_dynamic != 0).unwrap_or(false);
            if !is_dynamic {
                continue;
            }
            let accel = match e.force {
                Some(f) => {
                    let inv = e.rigid_body.map(|rb| rb.inverse_mass()).unwrap_or(1.0);
                    gravity + f.force * inv
                }
                None => gravity,
            };
            if let Some(v) = e.velocity.as_mut() {
                v.linear += accel * dt;
                v.linear = v.linear * self.config.linear_damping;
                v.angular = v.angular * self.config.angular_damping;
            } else {
                e.velocity = Some(Velocity {
                    linear: accel * dt,
                    angular: Vec3::ZERO,
                });
            }
            if let Some(f) = e.force.as_mut() {
                f.force = Vec3::ZERO;
                f.torque = Vec3::ZERO;
            }
        }

        // 2. Integrate velocity into position. Anti-tunneling: when the per-step
        // displacement exceeds half the body's smallest half-extent, advance in
        // equal substeps so contacts cannot be skipped in a single tick.
        for (_id, e) in scene.iter_mut() {
            let is_dynamic = e.rigid_body.map(|rb| rb.is_dynamic != 0).unwrap_or(false);
            if !is_dynamic {
                continue;
            }
            let max_step = e.collider.as_ref().map(min_half_extent).unwrap_or(0.0);
            if let (Some(t), Some(v)) = (e.transform.as_mut(), e.velocity.as_mut()) {
                let step = v.linear * dt;
                let step_len = step.length();
                if max_step > 0.0 && step_len > max_step {
                    let n = (step_len / max_step).ceil() as u32;
                    let sub = step * (1.0 / n as f64);
                    for _ in 0..n {
                        t.position += sub;
                    }
                } else {
                    t.position += step;
                }
                if v.angular != Vec3::ZERO {
                    // First-order angular update about the local axes.
                    let axis = v.angular.normalized();
                    let angle = v.angular.length() * dt;
                    t.rotation = t
                        .rotation
                        .composed(crate::math::Quat::from_axis_angle(axis, angle).normalized());
                    t.rotation = t.rotation.normalized();
                }
            }
        }

        // 3. Broad phase: uniform-grid candidates in stable sorted order.
        let bounds: Vec<(u128, Aabb)> = scene
            .iter()
            .filter_map(|(id, e)| {
                let t = e.transform?;
                let c = e.collider.as_ref()?;
                Some((id.0, c.world_aabb(t.position)))
            })
            .collect();
        let candidates: Vec<(u128, u128)> = match self.config.broad_phase {
            BroadPhaseKind::Grid => crate::broadphase::UniformGrid::build(&bounds).pairs(),
            BroadPhaseKind::Bvh => crate::broadphase::Bvh::build(&bounds).pairs(),
        };

        // 4. Narrow phase: shape-aware contacts (sphere/sphere, sphere/AABB, box/box).
        let lookup: std::collections::BTreeMap<u128, (Aabb, Collider)> = scene
            .iter()
            .filter_map(|(id, e)| {
                let t = e.transform?;
                let c = e.collider.as_ref()?;
                Some((id.0, (c.world_aabb(t.position), c.clone())))
            })
            .collect();
        let transforms: std::collections::BTreeMap<u128, Transform> = scene
            .iter()
            .filter_map(|(id, e)| e.transform.map(|t| (id.0, t)))
            .collect();
        for (ida, idb) in candidates {
            let (aabb_a, ca) = match lookup.get(&ida) {
                Some(v) => v.clone(),
                None => continue,
            };
            let (aabb_b, cb) = match lookup.get(&idb) {
                Some(v) => v.clone(),
                None => continue,
            };
            if !aabb_a.overlaps(aabb_b) {
                continue;
            }
            let ta = transforms[&ida];
            let tb = transforms[&idb];
            if let Some(p) = precise_contact(ta.position, &ca, tb.position, &cb) {
                self.resolve_contact(scene, ida, idb, p, contacts);
            }
        }

        // 4.5 Constraint solve: distance joints, iterated (position relaxation).
        for _ in 0..self.config.solver_iterations {
            self.solve_joints(scene);
        }

        // 5. Ground plane contact (y = 0) for dynamic bodies that fell below.
        let ground_hits: Vec<(u128, f64)> = scene
            .iter()
            .filter(|(_, e)| e.rigid_body.map(|rb| rb.is_dynamic != 0).unwrap_or(false))
            .filter_map(|(id, e)| {
                let collider = e.collider.as_ref()?;
                let transform = e.transform?;
                let aabb = collider.world_aabb(transform.position);
                if aabb.min.y < 0.0 {
                    Some((id.0, -aabb.min.y))
                } else {
                    None
                }
            })
            .collect();
        for (id, pen) in ground_hits {
            self.resolve_ground(scene, id, pen, contacts);
        }
    }

    fn resolve_contact(
        &self,
        scene: &mut Scene,
        ida: u128,
        idb: u128,
        contact_point: PreciseContact,
        contacts: &mut Vec<Contact>,
    ) {
        let PreciseContact {
            normal: dir, depth, ..
        } = contact_point;
        let point = contact_point.point;
        let dyn_a = scene
            .get(EntityId(ida))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.is_dynamic != 0)
            .unwrap_or(false);
        let dyn_b = scene
            .get(EntityId(idb))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.is_dynamic != 0)
            .unwrap_or(false);

        let (inv_a, inv_b) = dynamic_weights(scene, ida, idb, dyn_a, dyn_b);

        // Positional correction + impulse along the contact normal.
        let total_inv = inv_a + inv_b;
        if total_inv <= 0.0 {
            return;
        }
        let correction = depth * self.config.correction_factor;
        if dyn_a {
            if let Some(t) = scene
                .get_mut(EntityId(ida))
                .and_then(|e| e.transform.as_mut())
            {
                t.position += dir * (correction * inv_a / total_inv);
            }
        }
        if dyn_b {
            if let Some(t) = scene
                .get_mut(EntityId(idb))
                .and_then(|e| e.transform.as_mut())
            {
                t.position -= dir * (correction * inv_b / total_inv);
            }
        }

        // Impulse: resolve relative normal velocity.
        let vel_a = scene
            .get(EntityId(ida))
            .and_then(|e| e.velocity)
            .unwrap_or_default();
        let vel_b = scene
            .get(EntityId(idb))
            .and_then(|e| e.velocity)
            .unwrap_or_default();
        let rel = vel_a.linear - vel_b.linear;
        let vel_n = rel.dot(dir);
        let mut normal_impulse = 0.0;
        if vel_n < 0.0 {
            let restitution = scene
                .get(EntityId(ida))
                .and_then(|e| e.rigid_body)
                .map(|rb| rb.restitution)
                .unwrap_or(self.config.default_restitution)
                .min(
                    scene
                        .get(EntityId(idb))
                        .and_then(|e| e.rigid_body)
                        .map(|rb| rb.restitution)
                        .unwrap_or(self.config.default_restitution),
                );
            let imp = -(1.0 + restitution) * vel_n / total_inv;
            normal_impulse = imp;
            if dyn_a {
                if let Some(v) = scene
                    .get_mut(EntityId(ida))
                    .and_then(|e| e.velocity.as_mut())
                {
                    v.linear += dir * (imp * inv_a);
                }
            }
            if dyn_b {
                if let Some(v) = scene
                    .get_mut(EntityId(idb))
                    .and_then(|e| e.velocity.as_mut())
                {
                    v.linear -= dir * (imp * inv_b);
                }
            }
        }

        // Coulomb friction: damp the relative tangential velocity, clamped by
        // the normal impulse magnitude and a declared coefficient.
        let vel_a2 = scene
            .get(EntityId(ida))
            .and_then(|e| e.velocity)
            .unwrap_or_default();
        let vel_b2 = scene
            .get(EntityId(idb))
            .and_then(|e| e.velocity)
            .unwrap_or_default();
        let rel = vel_a2.linear - vel_b2.linear;
        let tangential = rel - dir * rel.dot(dir);
        if tangential.length_sq() > 1e-18 && normal_impulse > 0.0 {
            let friction = scene
                .get(EntityId(ida))
                .and_then(|e| e.rigid_body)
                .map(|rb| rb.friction)
                .unwrap_or(self.config.default_friction)
                .max(
                    scene
                        .get(EntityId(idb))
                        .and_then(|e| e.rigid_body)
                        .map(|rb| rb.friction)
                        .unwrap_or(self.config.default_friction),
                );
            let tangent = tangential.normalized();
            // Coulomb clamp: the friction impulse cannot exceed mu * |J_n|.
            let jt_raw = tangential.length() / total_inv;
            let jt = jt_raw.clamp(0.0, friction * normal_impulse);
            if dyn_a {
                if let Some(v) = scene
                    .get_mut(EntityId(ida))
                    .and_then(|e| e.velocity.as_mut())
                {
                    v.linear -= tangent * (jt * inv_a);
                }
            }
            if dyn_b {
                if let Some(v) = scene
                    .get_mut(EntityId(idb))
                    .and_then(|e| e.velocity.as_mut())
                {
                    v.linear += tangent * (jt * inv_b);
                }
            }
        }

        contacts.push(Contact {
            a: ida,
            b: idb,
            normal: dir,
            penetration: depth,
            point,
        });
    }

    /// Position relaxation of joints. Each joint is processed once, from the
    /// lower-id side, so results are deterministic.
    fn solve_joints(&self, scene: &mut Scene) {
        #[derive(Clone, Copy)]
        struct Pending {
            a: u128,
            b: u128,
            rest: f64,
            stiffness: f64,
            kind: JointKind,
            axis: Vec3,
            anchor: Vec3,
        }
        #[derive(Clone, Copy, PartialEq)]
        enum JointKind {
            Distance,
            Hinge,
            Prismatic,
        }

        let joints: Vec<Pending> = scene
            .iter()
            .filter_map(|(id, e)| {
                let j = e.joint?;
                let (rest, stiffness, kind, axis, anchor) = match j {
                    crate::components::Joint::Distance(d) => (
                        d.rest_length,
                        d.stiffness,
                        JointKind::Distance,
                        Vec3::ZERO,
                        Vec3::ZERO,
                    ),
                    crate::components::Joint::Hinge(h) => {
                        (0.0, 1.0, JointKind::Hinge, h.axis, h.anchor)
                    }
                    crate::components::Joint::Prismatic(p) => {
                        (0.0, 1.0, JointKind::Prismatic, p.axis, Vec3::ZERO)
                    }
                };
                if scene.get(j.other()).is_some() {
                    Some(Pending {
                        a: id.0,
                        b: j.other().0,
                        rest,
                        stiffness,
                        kind,
                        axis,
                        anchor,
                    })
                } else {
                    None
                }
            })
            .collect();

        for joint in joints {
            let pos_a = scene.position(EntityId(joint.a)).unwrap_or(Vec3::ZERO);
            let pos_b = scene.position(EntityId(joint.b)).unwrap_or(Vec3::ZERO);
            let stiffness = joint.stiffness.clamp(0.0, 1.0);
            match joint.kind {
                JointKind::Distance => {
                    let delta = pos_b - pos_a;
                    let dist = delta.length();
                    if dist < 1e-9 {
                        continue;
                    }
                    let error_val = dist - joint.rest;
                    let dir = delta * (1.0 / dist);
                    self.reposition(scene, joint.a, joint.b, dir, error_val, stiffness);
                }
                JointKind::Hinge => {
                    // Welded anchor: push both bodies to the midpoint of their
                    // respective anchor world positions.
                    let anchor_a = pos_a + joint.anchor;
                    let anchor_b = pos_b + joint.anchor;
                    let diff = anchor_a - anchor_b;
                    let error = diff.length();
                    if error < 1e-9 {
                        continue;
                    }
                    let dir = diff.normalized();
                    self.reposition(scene, joint.a, joint.b, dir, error, stiffness.max(0.5));
                }
                JointKind::Prismatic => {
                    // Remove the perpendicular component of the relative
                    // position so motion stays on the shared axis.
                    let delta = pos_b - pos_a;
                    let axis = joint.axis.normalized();
                    if axis.length_sq() < 1e-12 {
                        continue;
                    }
                    let along = delta.dot(axis);
                    let perpendicular = delta - axis * along;
                    let error_val = perpendicular.length();
                    if error_val < 1e-9 {
                        continue;
                    }
                    let dir = perpendicular * (1.0 / error_val);
                    self.reposition(scene, joint.a, joint.b, dir, error_val, stiffness.max(0.5));
                }
            }
        }
    }

    /// Bidirectional position correction of A and B along `dir` by `error`.
    fn reposition(
        &self,
        scene: &mut Scene,
        a: u128,
        b: u128,
        dir: Vec3,
        error: f64,
        stiffness: f64,
    ) {
        let inv_a = scene
            .get(EntityId(a))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.inverse_mass())
            .unwrap_or(0.0);
        let inv_b = scene
            .get(EntityId(b))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.inverse_mass())
            .unwrap_or(0.0);
        let total = inv_a + inv_b;
        if total <= 0.0 {
            return;
        }
        let corr = dir * (error * stiffness);
        if inv_a > 0.0 {
            if let Some(t) = scene
                .get_mut(EntityId(a))
                .and_then(|e| e.transform.as_mut())
            {
                t.position += corr * (inv_a / total);
            }
        }
        if inv_b > 0.0 {
            if let Some(t) = scene
                .get_mut(EntityId(b))
                .and_then(|e| e.transform.as_mut())
            {
                t.position -= corr * (inv_b / total);
            }
        }
    }

    fn resolve_ground(&self, scene: &mut Scene, id: u128, pen: f64, contacts: &mut Vec<Contact>) {
        let is_dynamic = scene
            .get(EntityId(id))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.is_dynamic != 0)
            .unwrap_or(false);
        if !is_dynamic {
            return;
        }
        let restitution = scene
            .get(EntityId(id))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.restitution)
            .unwrap_or(self.config.default_restitution);
        if let Some(t) = scene
            .get_mut(EntityId(id))
            .and_then(|e| e.transform.as_mut())
        {
            t.position.y += pen;
        }
        if let Some(v) = scene
            .get_mut(EntityId(id))
            .and_then(|e| e.velocity.as_mut())
        {
            if v.linear.y < 0.0 {
                v.linear.y = -v.linear.y * restitution;
                // Apply tangential friction.
                v.linear.x *= 0.98;
                v.linear.z *= 0.98;
            }
        }
        contacts.push(Contact {
            a: id,
            b: GROUND,
            normal: Vec3::new(0.0, 1.0, 0.0),
            penetration: pen,
            point: scene
                .get(EntityId(id))
                .and_then(|e| e.transform)
                .map(|t| t.position)
                .unwrap_or(Vec3::ZERO),
        });
    }
}

/// A precise contact between two shapes: normal (B→A), penetration depth, point.
#[derive(Clone, Copy, Debug)]
struct PreciseContact {
    normal: Vec3,
    depth: f64,
    point: Vec3,
}

/// Shape-aware contact computation: sphere/sphere, sphere/box, box/box,
/// heightfield, compound, and convex hull (GJK+EPA). Returns the contact with
/// greatest penetration.
fn precise_contact(pa: Vec3, ca: &Collider, pb: Vec3, cb: &Collider) -> Option<PreciseContact> {
    match (ca, cb) {
        (
            Collider::Sphere {
                radius: ra,
                offset: oa,
            },
            Collider::Sphere {
                radius: rb,
                offset: ob,
            },
        ) => {
            let ca = pa + *oa;
            let cb = pb + *ob;
            let delta = cb - ca;
            let dist = delta.length();
            let sum = ra + rb;
            if dist < sum {
                let n = if dist > 1e-9 {
                    delta * (1.0 / dist)
                } else {
                    Vec3::new(0.0, 1.0, 0.0)
                };
                Some(PreciseContact {
                    normal: -n,
                    depth: sum - dist,
                    point: ca + n * (ra - (sum - dist) * 0.5),
                })
            } else {
                None
            }
        }
        (Collider::Sphere { radius, offset }, Collider::Box { dims, offset: o }) => {
            sphere_box(pa + *offset, *radius, pb + *o, *dims).map(|mut c| {
                c.normal = -c.normal;
                c
            })
        }
        (Collider::Box { dims, offset }, Collider::Sphere { radius, offset: o }) => {
            sphere_box(pb + *o, *radius, pa + *offset, *dims)
        }
        (
            Collider::Box { dims, offset: oa },
            Collider::Box {
                dims: dims_b,
                offset: ob,
            },
        ) => box_box(pa + *oa, *dims, pb + *ob, *dims_b),
        // Compound: take child contacts against the other shape.
        (Collider::Compound(sub_a), other) => {
            let mut best: Option<PreciseContact> = None;
            for child in sub_a {
                if let Some(c) = precise_contact(pa, child, pb, other) {
                    best = Some(match best {
                        None => c,
                        Some(prev) if c.depth > prev.depth => c,
                        Some(prev) => prev,
                    });
                }
            }
            best
        }
        (other, Collider::Compound(sub_b)) => {
            let mut best: Option<PreciseContact> = None;
            for child in sub_b {
                if let Some(c) = precise_contact(pa, other, pb, child) {
                    best = Some(match best {
                        None => c,
                        Some(prev) if c.depth > prev.depth => c,
                        Some(prev) => prev,
                    });
                }
            }
            best
        }
        // Heightfield terrain: nearest terrain sample under the shape.
        (Collider::Heightfield(hf), other) => heightfield_contact(pb, other, hf).map(|mut c| {
            c.normal = -c.normal; // flip since we passed 'a' as heightfield
            c
        }),
        (other, Collider::Heightfield(hf)) => heightfield_contact(pa, other, hf),
        // Convex hull: GJK + EPA against any primitive (hull, box, sphere).
        (Collider::ConvexHull { points }, other) => hull_contact(pa, points, pb, other),
        (other, Collider::ConvexHull { points }) => {
            hull_contact(pb, points, pa, other).map(|mut c| {
                c.normal = -c.normal;
                c
            })
        }
    }
}

/// SAT (separating axis theorem) contact between a convex hull and any
/// primitive. For convex polyhedra SAT is exact: it returns the minimal
/// translation to separate the pair, with the normal pointing B→A (matching the
/// other narrow-phase cases). Deterministic: candidate axes are generated in a
/// fixed order and the minimum-penetration axis wins.
fn hull_contact(pa: Vec3, hull: &[Vec3], pb: Vec3, other: &Collider) -> Option<PreciseContact> {
    let a_world: Vec<Vec3> = hull.iter().map(|p| pa + *p).collect();
    match other {
        Collider::Box { dims, offset } => {
            let mut b_world = Vec::with_capacity(8);
            for (sx, sy, sz) in [
                (-1.0, -1.0, -1.0),
                (1.0, -1.0, -1.0),
                (-1.0, 1.0, -1.0),
                (1.0, 1.0, -1.0),
                (-1.0, -1.0, 1.0),
                (1.0, -1.0, 1.0),
                (-1.0, 1.0, 1.0),
                (1.0, 1.0, 1.0),
            ] {
                b_world.push(pb + Vec3::new(sx * dims.x, sy * dims.y, sz * dims.z) + *offset);
            }
            sat_contact(&a_world, &b_world, None)
        }
        Collider::ConvexHull { points } => {
            let b_world: Vec<Vec3> = points.iter().map(|p| pb + *p).collect();
            sat_contact(&a_world, &b_world, None)
        }
        Collider::Sphere { radius, offset } => {
            sat_contact(&a_world, &[], Some((pb + *offset, *radius)))
        }
        _ => None,
    }
}

/// Runs SAT between two convex shapes. `b_points` are the world-space vertices
/// of a polyhedron, or `b_sphere = Some((center, radius))` for a sphere.
fn sat_contact(
    a_world: &[Vec3],
    b_points: &[Vec3],
    b_sphere: Option<(Vec3, f64)>,
) -> Option<PreciseContact> {
    // Deterministic axis set: the three coordinate axes (faces of any
    // axis-aligned box/hull) plus the SAT edge axes — every vertex-pair
    // difference and every cross product of an a-edge with a b-edge.
    let mut raw_axes: Vec<Vec3> = vec![
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ];
    for i in 0..a_world.len() {
        for j in (i + 1)..a_world.len() {
            raw_axes.push(a_world[i] - a_world[j]);
        }
    }
    for i in 0..b_points.len() {
        for j in (i + 1)..b_points.len() {
            raw_axes.push(b_points[i] - b_points[j]);
        }
    }
    let b_axis_ref: Vec<Vec3> = match b_sphere {
        Some((center, _)) => vec![center],
        None => b_points.to_vec(),
    };
    for a in a_world {
        for b in &b_axis_ref {
            raw_axes.push(a.cross(*b));
        }
    }

    // Normalize and de-duplicate axes deterministically.
    let mut axes: Vec<Vec3> = Vec::new();
    for ax in raw_axes {
        let n = ax.normalized();
        if n.length_sq() < 1e-12 {
            continue;
        }
        let exists = axes.iter().any(|e| e.dot(n).abs() > 0.9999);
        if !exists {
            axes.push(n);
        }
    }

    let mut best: Option<(Vec3, f64)> = None;
    for axis in axes {
        let (min_a, max_a) = project(a_world, axis);
        let (min_b, max_b) = match b_sphere {
            Some((center, radius)) => project_sphere(center, radius, axis),
            None => project(b_points, axis),
        };
        if max_a < min_b || max_b < min_a {
            return None; // separating axis
        }
        let overlap = (max_a - min_b).min(max_b - min_a);
        if best.map_or(true, |(_, d)| overlap < d) {
            best = Some((axis, overlap));
        }
    }
    let (mut axis, depth) = best?;

    // Orient the normal B→A so resolving pushes B out of A.
    let ca = centroid(a_world);
    let cb = match b_sphere {
        Some((center, _)) => center,
        None => centroid(b_points),
    };
    if (cb - ca).dot(axis) > 0.0 {
        axis = axis.scale(-1.0);
    }
    let point = ca + axis * depth * 0.5;
    Some(PreciseContact {
        normal: axis,
        depth,
        point,
    })
}

fn centroid(points: &[Vec3]) -> Vec3 {
    let sum = points.iter().fold(Vec3::ZERO, |acc, p| acc + *p);
    sum * (1.0 / points.len() as f64)
}

/// Project `points` onto `axis`, returning (min, max).
fn project(points: &[Vec3], axis: Vec3) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for p in points {
        let d = p.dot(axis);
        if d < min {
            min = d;
        }
        if d > max {
            max = d;
        }
    }
    (min, max)
}

/// Project a sphere (center, radius) onto `axis`.
fn project_sphere(center: Vec3, radius: f64, axis: Vec3) -> (f64, f64) {
    let c = center.dot(axis);
    (c - radius, c + radius)
}
/// Box/box baseline: minimum-axis penetration. Normal points B→A.
fn box_box(pa: Vec3, da: Vec3, pb: Vec3, db: Vec3) -> Option<PreciseContact> {
    let aa = Aabb::from_center_half(pa, da);
    let bb = Aabb::from_center_half(pb, db);
    if !aa.overlaps(bb) {
        return None;
    }
    let pen = aa.penetration(bb);
    let (axis, depth) = if pen.x <= pen.y && pen.x <= pen.z {
        (Vec3::new(1.0, 0.0, 0.0), pen.x)
    } else if pen.y <= pen.z {
        (Vec3::new(0.0, 1.0, 0.0), pen.y)
    } else {
        (Vec3::new(0.0, 0.0, 1.0), pen.z)
    };
    let dir = if center_component(aa.center(), axis) < center_component(bb.center(), axis) {
        -axis
    } else {
        axis
    };
    Some(PreciseContact {
        normal: dir,
        depth,
        point: (aa.center() + bb.center()) * 0.5,
    })
}

/// Heightfield-vs-shape contact. Computes contact against the terrain height
/// directly under the shape (sphere or box lowest point). Depth is capped so
/// bodies occasionally rest on terrain without being ejected.
fn heightfield_contact(pos: Vec3, shape: &Collider, hf: &Heightfield) -> Option<PreciseContact> {
    let (c_x, _c_y, c_z, bottom_y) = match shape {
        Collider::Sphere { radius, offset } => {
            let c = pos + *offset;
            (c.x, c.y, c.z, c.y - *radius)
        }
        Collider::Box { dims, offset } => {
            let c = pos + *offset;
            (c.x, c.y, c.z, c.y - dims.y)
        }
        Collider::Compound(_) | Collider::Heightfield(_) | Collider::ConvexHull { .. } => {
            return None
        }
    };
    let terrain = hf.height_at(c_x, c_z);
    if bottom_y < terrain {
        // Gradient-based normal from the nearest samples in x and z.
        let hx = hf.height_at(c_x + hf.col_step, c_z) - hf.height_at(c_x - hf.col_step, c_z);
        let hz = hf.height_at(c_x, c_z + hf.row_step) - hf.height_at(c_x, c_z - hf.row_step);
        let normal = Vec3::new(-hx, 2.0 * hf.col_step, -hz).normalized();
        Some(PreciseContact {
            normal,
            depth: (terrain - bottom_y).min(1.0),
            point: Vec3::new(c_x, terrain, c_z),
        })
    } else {
        None
    }
}

/// Closest-point sphere/AABB contact. Normal points from sphere → box away.
fn sphere_box(c_s: Vec3, r: f64, c_b: Vec3, half: Vec3) -> Option<PreciseContact> {
    let clamped = Vec3::new(
        c_s.x.clamp(c_b.x - half.x, c_b.x + half.x),
        c_s.y.clamp(c_b.y - half.y, c_b.y + half.y),
        c_s.z.clamp(c_b.z - half.z, c_b.z + half.z),
    );
    let delta = clamped - c_s;
    let d = delta.length();
    if d >= r {
        return None;
    }
    let n = if d > 1e-9 {
        delta * (1.0 / d)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    Some(PreciseContact {
        normal: n,
        depth: r - d,
        point: clamped,
    })
}

fn center_component(v: Vec3, axis: Vec3) -> f64 {
    if axis.x != 0.0 {
        v.x
    } else if axis.y != 0.0 {
        v.y
    } else {
        v.z
    }
}

/// Smallest per-axis half-extent of a collider (used for anti-tunneling).
fn min_half_extent(c: &Collider) -> f64 {
    match c {
        Collider::Box { dims, .. } => dims.x.min(dims.y).min(dims.z) * 0.5,
        Collider::Sphere { radius, .. } => radius * 0.5,
        Collider::Compound(parts) => parts
            .iter()
            .map(min_half_extent)
            .fold(f64::INFINITY, f64::min),
        Collider::Heightfield(hf) => hf.col_step.min(hf.row_step) * 0.5,
        Collider::ConvexHull { points } => points
            .iter()
            .map(|p| p.x.abs().min(p.y.abs()).min(p.z.abs()))
            .fold(f64::INFINITY, f64::min),
    }
}

fn dynamic_weights(scene: &Scene, ida: u128, idb: u128, dyn_a: bool, dyn_b: bool) -> (f64, f64) {
    let inv_a = if dyn_a {
        scene
            .get(EntityId(ida))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.inverse_mass())
            .unwrap_or(1.0)
    } else {
        0.0
    };
    let inv_b = if dyn_b {
        scene
            .get(EntityId(idb))
            .and_then(|e| e.rigid_body)
            .map(|rb| rb.inverse_mass())
            .unwrap_or(1.0)
    } else {
        0.0
    };
    (inv_a, inv_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Collider, DistanceJoint, Joint, RigidBody, Transform};
    use crate::scene::Entity;

    fn falling_body(scene: &mut Scene, id: u128, height: f64) {
        let mut e = Entity::dynamic();
        e.transform = Some(Transform {
            position: Vec3::new(0.0, height, 0.0),
            ..Default::default()
        });
        e.velocity = Some(Velocity::default());
        e.rigid_body = Some(RigidBody::dynamic(1.0));
        e.collider = Some(Collider::sphere(0.5));
        scene.insert(EntityId(id), e);
    }

    #[test]
    fn distance_joint_holds_bodies_together() {
        let mut scene = Scene::new(Vec3::ZERO); // no gravity: isolate the joint
        let ra = RigidBody::dynamic(1.0);
        let rb2 = RigidBody::dynamic(1.0);
        let mut a = Entity::dynamic();
        a.transform = Some(Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Default::default()
        });
        a.rigid_body = Some(ra);
        a.joint = Some(Joint::Distance(DistanceJoint {
            other: EntityId(2),
            rest_length: 1.0,
            stiffness: 0.5,
        }));
        let mut b = Entity::dynamic();
        b.transform = Some(Transform {
            position: Vec3::new(0.0, 8.0, 0.0),
            ..Default::default()
        });
        b.rigid_body = Some(rb2);
        scene.insert(EntityId(1), a);
        scene.insert(EntityId(2), b);

        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..60 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let d = scene
            .position(EntityId(1))
            .unwrap()
            .distance(scene.position(EntityId(2)).unwrap());
        assert!(
            (d - 1.0).abs() < 0.05,
            "joint distance {d} should rest ~1.0"
        );
    }

    #[test]
    fn sphere_sphere_collision_separates() {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut a = Entity::dynamic();
        a.transform = Some(Transform {
            position: Vec3::new(-0.6, 0.5, 0.0),
            ..Default::default()
        });
        a.velocity = Some(Velocity {
            linear: Vec3::new(1.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        a.rigid_body = Some(RigidBody::dynamic(1.0));
        a.collider = Some(Collider::sphere(0.5));
        let mut b = Entity::dynamic();
        b.transform = Some(Transform {
            position: Vec3::new(0.6, 0.5, 0.0),
            ..Default::default()
        });
        b.rigid_body = Some(RigidBody::r#static());
        b.collider = Some(Collider::sphere(0.5));
        scene.insert(EntityId(1), a);
        scene.insert(EntityId(2), b);

        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..30 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        // The moving sphere must be stopped/pushed back by the static one.
        assert!(
            scene.position(EntityId(1)).unwrap().x <= -0.1,
            "sphere should not penetrate"
        );
        assert!(!contacts.is_empty(), "contact expected");
    }

    #[test]
    fn fast_body_does_not_tunnel_through_ground() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut fast = Entity::dynamic();
        fast.transform = Some(Transform {
            position: Vec3::new(0.0, 2.0, 0.0),
            ..Default::default()
        });
        // Extreme downward velocity: 600 m/s would tunnel 10m per 60Hz step.
        fast.velocity = Some(Velocity {
            linear: Vec3::new(0.0, -600.0, 0.0),
            angular: Vec3::ZERO,
        });
        fast.rigid_body = Some(RigidBody::dynamic(1.0));
        fast.collider = Some(Collider::sphere(0.25));
        scene.insert(EntityId(9), fast);
        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..120 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let y = scene.position(EntityId(9)).unwrap().y;
        assert!(y >= 0.0, "fast body tunneled to y={y}");
    }

    #[test]
    fn sphere_rests_on_flat_terrain_heightfield() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut field = Entity::dynamic();
        field.rigid_body = Some(RigidBody::r#static());
        field.collider = Some(Collider::heightfield(crate::components::Heightfield::new(
            64,
            64,
            1.0,
            1.0,
            Vec3::new(-32.0, 0.0, -32.0),
        )));
        scene.insert(EntityId(5), field);

        let mut ball = Entity::dynamic();
        ball.transform = Some(Transform {
            position: Vec3::new(0.0, 3.0, 0.0),
            ..Default::default()
        });
        ball.velocity = Some(Velocity::default());
        ball.rigid_body = Some(RigidBody::dynamic(1.0));
        ball.collider = Some(Collider::sphere(0.5));
        scene.insert(EntityId(1), ball);

        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..300 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let y = scene.position(EntityId(1)).unwrap().y;
        // The ball lands on the flat terrain height (0) and centers itself
        // slightly above, since the contact clamps depth.
        assert!(
            y > 0.0 && y < 3.0,
            "final ball y={y} outside expected range"
        );
        assert!(y >= 0.0 + 0.45, "ball should sit on the terrain, y={y}");
    }

    #[test]
    fn compound_collider_hits_interacting_shapes() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut ground = Entity::dynamic();
        ground.rigid_body = Some(RigidBody::r#static());
        ground.collider = Some(Collider::aabb(Vec3::new(50.0, 5.0, 50.0)));
        ground.transform = Some(Transform {
            position: Vec3::new(0.0, -5.0, 0.0),
            ..Default::default()
        });
        scene.insert(EntityId(0), ground);

        // Compound: we chuck two spheres in one collider, side by side.
        let wheel_a = Collider::Sphere {
            radius: 0.3,
            offset: Vec3::new(-1.0, -1.0, 0.0),
        };
        let wheel_b = Collider::Sphere {
            radius: 0.3,
            offset: Vec3::new(1.0, -1.0, 0.0),
        };
        let chassis = Collider::Box {
            dims: Vec3::new(2.0, 0.5, 1.0),
            offset: Vec3::ZERO,
        };
        let mut rig = Entity::dynamic();
        rig.transform = Some(Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Default::default()
        });
        rig.rigid_body = Some(RigidBody::r#dynamic(2.0));
        rig.collider = Some(Collider::Compound(vec![chassis, wheel_a, wheel_b]));
        scene.insert(EntityId(2), rig);

        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..600 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let y = scene.position(EntityId(2)).unwrap().y;
        // The compound rig lands so that wheels touch ground: wheel center at
        // y = 0.3, chassis bottom at y = chassis_center - 0.5 = rest.
        assert!(y > 0.0 && y < 3.0, "compound rig final y={y} unexpected");
    }

    #[test]
    fn buggy_rolls_down_a_slope_and_departs() {
        // Sloped heightfield: y = -0.2 * x. Buggy starts on the slope.
        let mut field = Entity::dynamic();
        field.rigid_body = Some(RigidBody::r#static());
        let mut hf = Heightfield::new(64, 64, 1.0, 1.0, Vec3::new(-10.0, 0.0, -5.0));
        for r in 0..hf.rows as usize {
            for c in 0..hf.cols as usize {
                let x = c as f64 - 10.0;
                hf.heights[r * 64 + c] = -0.2 * x;
            }
        }
        field.collider = Some(Collider::heightfield(hf));
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        scene.insert(EntityId(0), field);

        // Buggy starts resting on the slope, slightly raised.
        let mut buggy = Entity::dynamic();
        buggy.transform = Some(Transform {
            position: Vec3::new(-4.0, 2.0, 0.0),
            ..Default::default()
        });
        buggy.velocity = Some(Velocity {
            linear: Vec3::new(1.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        });
        buggy.rigid_body = Some(RigidBody {
            mass: 10.0,
            restitution: 0.0,
            friction: 0.3,
            is_dynamic: 1,
        });
        buggy.collider = Some(Collider::sphere(0.5));
        scene.insert(EntityId(2), buggy);

        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..240 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let p = scene.position(EntityId(2)).unwrap();
        // The buggy must have moved downhill (positive x) after rolling on the slope.
        assert!(p.x > -3.5, "buggy should have rolled downhill: {:?}", p);
        assert!(p.y < 2.0, "buggy should have descended: {:?}", p);
    }

    #[test]
    fn body_falls_under_gravity() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        falling_body(&mut scene, 1, 10.0);
        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..10 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let y = scene.position(EntityId(1)).unwrap().y;
        // A body falling for 10 frames must be below its start.
        assert!(y < 10.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn body_rests_on_ground_not_tunneling() {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        falling_body(&mut scene, 1, 5.0);
        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut contacts = Vec::new();
        for _ in 0..600 {
            physics.step(&mut scene, 1.0 / 60.0, &mut contacts);
        }
        let y = scene.position(EntityId(1)).unwrap().y;
        // After resting, the sphere bottom should sit at y=0 (center ~ radius).
        assert!((y - 0.5).abs() < 0.01, "y = {y}");
    }

    fn unit_cube_hull() -> Collider {
        let pts = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ]
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]) * 0.5)
        .collect();
        Collider::convex_hull(pts)
    }

    #[test]
    fn convex_hull_penetration_matches_analytic() {
        // Two unit cubes centered 0.5 apart overlap by 0.5 along x.
        let a = unit_cube_hull();
        let b = unit_cube_hull();
        let c = precise_contact(Vec3::new(0.0, 0.0, 0.0), &a, Vec3::new(0.5, 0.0, 0.0), &b)
            .expect("overlapping hulls must contact");
        assert!((c.depth - 0.5).abs() < 1e-3, "depth = {}", c.depth);
        // Normal points B→A, i.e. roughly along -x.
        assert!(c.normal.x < -0.9, "normal = {:?}", c.normal);
    }

    #[test]
    fn convex_hull_contact_is_symmetric() {
        let a = unit_cube_hull();
        let b = unit_cube_hull();
        let ab =
            precise_contact(Vec3::new(0.0, 0.0, 0.0), &a, Vec3::new(0.3, 0.2, 0.0), &b).unwrap();
        let ba =
            precise_contact(Vec3::new(0.3, 0.2, 0.0), &b, Vec3::new(0.0, 0.0, 0.0), &a).unwrap();
        assert!((ab.depth - ba.depth).abs() < 1e-3);
        // Normals are opposite (B→A vs A→B).
        assert!(ab.normal.dot(ba.normal) < -0.9);
    }

    #[test]
    fn convex_hull_vs_sphere_and_remoteness() {
        let a = unit_cube_hull();
        let sphere = Collider::sphere(0.5);
        // Sphere overlapping the hull face.
        let c = precise_contact(
            Vec3::new(0.0, 0.0, 0.0),
            &a,
            Vec3::new(0.75, 0.0, 0.0),
            &sphere,
        )
        .expect("sphere overlapping hull face must contact");
        assert!(c.depth > 0.0);
        // A far-away sphere does not contact.
        assert!(precise_contact(
            Vec3::new(0.0, 0.0, 0.0),
            &a,
            Vec3::new(10.0, 0.0, 0.0),
            &sphere,
        )
        .is_none());
    }

    #[test]
    fn convex_hull_contact_is_deterministic() {
        let a = unit_cube_hull();
        let b = Collider::sphere(0.4);
        let one = precise_contact(Vec3::ZERO, &a, Vec3::new(0.6, 0.1, 0.0), &b).unwrap();
        let two = precise_contact(Vec3::ZERO, &a, Vec3::new(0.6, 0.1, 0.0), &b).unwrap();
        assert_eq!(one.depth, two.depth);
        assert_eq!(one.normal, two.normal);
        assert_eq!(one.point, two.point);
    }

    #[test]
    fn simulation_is_deterministic() {
        let mut a = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut b = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        for id in [1u128, 2, 3] {
            falling_body(&mut a, id, 3.0 + id as f64);
            falling_body(&mut b, id, 3.0 + id as f64);
        }
        let physics = PhysicsSystem::new(PhysicsConfig::default());
        let mut c1 = Vec::new();
        let mut c2 = Vec::new();
        for _ in 0..100 {
            physics.step(&mut a, 1.0 / 60.0, &mut c1);
            physics.step(&mut b, 1.0 / 60.0, &mut c2);
        }
        for id in [1u128, 2, 3] {
            assert_eq!(a.position(EntityId(id)), b.position(EntityId(id)));
        }
    }

    #[test]
    fn broad_phase_backends_produce_identical_simulation() {
        // The uniform grid and BVH are interchangeable: switching the backend
        // must not change a single simulated position.
        let make_scene = || {
            let mut s = Scene::new(Vec3::new(0.0, -9.81, 0.0));
            for id in [1u128, 2, 3, 4] {
                falling_body(&mut s, id, 2.0 + id as f64 * 0.7);
            }
            s
        };
        let mut grid_scene = make_scene();
        let mut bvh_scene = make_scene();
        let grid = PhysicsSystem::new(PhysicsConfig {
            broad_phase: BroadPhaseKind::Grid,
            ..PhysicsConfig::default()
        });
        let bvh = PhysicsSystem::new(PhysicsConfig {
            broad_phase: BroadPhaseKind::Bvh,
            ..PhysicsConfig::default()
        });
        let mut cg = Vec::new();
        let mut cb = Vec::new();
        for _ in 0..150 {
            grid.step(&mut grid_scene, 1.0 / 60.0, &mut cg);
            bvh.step(&mut bvh_scene, 1.0 / 60.0, &mut cb);
        }
        for id in [1u128, 2, 3, 4] {
            assert_eq!(
                grid_scene.position(EntityId(id)),
                bvh_scene.position(EntityId(id)),
                "broad-phase backends diverged for entity {id}"
            );
        }
    }
}
