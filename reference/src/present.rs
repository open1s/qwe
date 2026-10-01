//! Presentation layer: turn a running simulation's `Scene` into 3D web frames.
//!
//! [`snapshot`] captures the world state (entity transforms + collider shapes +
//! state slots + channels + camera) into a [`PresentationFrame`];
//! [`frame_to_json`] serializes frames, and [`write_viewer`] emits one
//! self-contained HTML file — three.js vendored into the source tree and
//! embedded inline — that plays them back offline; double-click the file, no
//! server or network needed. [`serve_live`] runs the same viewport against a
//! live runtime over HTTP.
//!
//! The viewer is generated output, not a build dependency. This makes a running
//! simulation visibly observable, including a micro/macro world at any scale.

use crate::math::{Quat, Vec3};
use crate::scene::Scene;
use std::fmt::Write as _;

/// A renderable collider shape (kind + parameters).
#[derive(Clone, Debug)]
pub enum Shape {
    Box {
        dims: Vec3,
    },
    Sphere {
        radius: f64,
    },
    Hull {
        points: Vec<Vec3>,
    },
    /// A smooth (optionally tapered) capsule along the local Y axis: rounded
    /// ends, radii `radius_bottom` -> `radius_top`. Ideal for limbs/fingers.
    Capsule {
        radius_bottom: f64,
        length: f64,
        radius_top: f64,
    },
    /// A user-defined custom shape: primitive parts with local offsets.
    Group(Vec<Part>),
    /// A torus (a `ring` part) — a shell/orbit ring, e.g. an electron shell.
    Ring {
        radius: f64,
        tube: f64,
    },
    /// A cylinder along the local Y axis: pillars, columns, rollers.
    Cylinder {
        radius: f64,
        height: f64,
    },
    /// A cone along the local Y axis: tips, spikes, funnels.
    Cone {
        radius: f64,
        height: f64,
    },
    /// A flat rectangle in the local XY plane (no thickness; double-sided in
    /// the viewer): panels, screens, ground quads.
    Plane {
        width: f64,
        height: f64,
    },
    /// An SVG path (`d`) extruded along Z into a 3D solid, then scaled.
    Svg {
        path: String,
        depth: f64,
        scale: f64,
    },
    /// An arbitrary polyhedron: vertices + vertex-index faces.
    Poly {
        points: Vec<Vec3>,
        faces: Vec<Vec<u32>>,
    },
    /// No collider: a point marker (e.g. a channel or a state-only body).
    Point,
}

/// A render bond between two atoms (molecules): the baseline is a line/stick;
/// `order` draws parallel sticks (single/double/triple), `polarity` tints it, and
/// `cloud` adds a translucent shared-electron region.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bond {
    pub a: u128,
    pub b: u128,
    pub order: u8,
    pub polarity: f64,
    pub cloud: bool,
    /// Draw only when the entities are within `[min, max]` distance.
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Bond {
    pub fn single(a: u128, b: u128) -> Self {
        Self {
            a,
            b,
            order: 1,
            polarity: 0.0,
            cloud: false,
            min: None,
            max: None,
        }
    }
}

/// One part of a composite (`group`) shape: a primitive with a local offset and
/// an optional per-part colour and animation.
#[derive(Clone, Debug)]
pub struct Part {
    pub shape: Shape,
    pub offset: Vec3,
    pub color: Option<u32>,
    /// Per-part opacity in `[0, 1]`; `None` = the entity's opacity.
    pub opacity: Option<f64>,
    pub orbit: Option<Orbit>,
    pub spin: f64,
}

/// A part's circular orbit about the entity's local origin (electrons around a
/// nucleus): position is `radius·(cos θ, sin θ)` in the plane normal to `axis`,
/// with `θ = phase + speed·t`.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub radius: f64,
    pub speed: f64,
    pub phase: f64,
    pub axis: Vec3,
}

/// One visible entity in a frame.
#[derive(Clone, Debug)]
pub struct EntityVisual {
    pub id: u128,
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub shape: Shape,
    pub state: Vec<f64>,
    /// Presentation color `0xRRGGBB`; falls back to a per-id hash when unset.
    pub color: u32,
    /// Visual size for state-only bodies (derived from mass).
    pub size: f64,
    /// Material opacity in `[0, 1]` (language `opacity` attribute; default 1).
    pub opacity: f64,
    /// Emissive glow intensity (language `glow` attribute; default 0.8).
    pub glow: f64,
    /// Whether to draw the floating name label (language `label`; default true).
    pub label: bool,
    /// When true, suppress the velocity arrow / orbit ring for this entity.
    pub no_velocity: bool,
    /// Optional material overrides sent to the viewer as the `vis` JSON block.
    /// Presentation-only; omitted entirely when unset, so older viewers and
    /// exact-JSON expectations keep working (backwards compatible).
    pub style: Option<VisualStyle>,
}

/// Per-entity material overrides for the viewer (the optional `vis` block).
/// Purely presentational — never affects simulation state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisualStyle {
    /// Metalness in `[0, 1]` (default 0.1 in the viewer).
    pub metalness: Option<f64>,
    /// Roughness in `[0, 1]` (default 0.55 in the viewer).
    pub roughness: Option<f64>,
    /// Emissive color `0xRRGGBB`; defaults to the entity color.
    pub emissive: Option<u32>,
    /// Emissive intensity; defaults to the entity `glow`.
    pub emissive_intensity: Option<f64>,
    /// Draw the geometry as wireframe.
    pub wireframe: Option<bool>,
    /// Flat shading (faceted look) instead of smooth normals.
    pub flat: Option<bool>,
    /// Render both faces (for open surfaces such as `plane`).
    pub double_sided: Option<bool>,
    /// `false` opts this entity's meshes out of shadow casting.
    pub cast_shadow: Option<bool>,
}

impl EntityVisual {
    /// Deterministic color for an entity id (used when the scene has none).
    fn color_for(id: u128) -> u32 {
        let h = (id.wrapping_mul(0x9E37_79B9) ^ 0x5F37_7F4A) & 0xFFFFFF;
        (h as u32) | 0xFF00_0000
    }
    /// The RGB `0xRRGGBB` value as a JSON number.
    fn color_value(&self) -> u32 {
        self.color & 0xFF_FFFF
    }
}

/// A visible size for a state-only body, derived from its mass (slot 6) so
/// heavier bodies (gas giants, the Sun) look bigger than rocky planets / the
/// Moon. `size = 0.35 · mass^0.3`, clamped.
fn visual_size(state: &[f64]) -> f64 {
    let mass = state.get(6).copied().unwrap_or(1.0).abs().max(1e-6);
    let s = 0.35 * mass.powf(0.3);
    s.clamp(0.06, 2.5)
}

/// A channel's live value (its mailbox `state[0]`).
#[derive(Clone, Copy, Debug)]
pub struct ChannelVisual {
    pub id: u128,
    pub value: f64,
}

/// A camera for the viewer: eye position and look-at target.
#[derive(Clone, Copy, Debug)]
pub struct CameraVisual {
    pub position: Vec3,
    pub target: Vec3,
}

/// A grid field sent to the viewer. The viewer draws it as a marching-cubes
/// isosurface: a spherical wavefront reads as a translucent shell that grows
/// outward and fades. For large grids `stride > 1` subsamples the linear
/// `[k][j][i]` stream so at most `MAX_FIELD_POINTS` cells are sent per frame.
#[derive(Clone, Debug)]
pub struct FieldVisual {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub dx: f64,
    pub stride: usize,
    /// Sampled cell values, in increasing linear-index order.
    pub cells: Vec<f64>,
}

/// At most this many field cells ride in one presentation frame.
const MAX_FIELD_POINTS: usize = 4096;

/// An immutable presentation snapshot of one world state.
#[derive(Clone, Debug, Default)]
pub struct PresentationFrame {
    pub time: f64,
    pub entities: Vec<EntityVisual>,
    pub channels: Vec<ChannelVisual>,
    pub fields: Vec<FieldVisual>,
    pub camera: Option<CameraVisual>,
    /// Render bonds between atoms (molecules): order/polarity/cloud.
    pub bonds: Vec<Bond>,
}

impl PresentationFrame {
    pub fn empty(time: f64) -> Self {
        Self {
            time,
            entities: Vec::new(),
            channels: Vec::new(),
            fields: Vec::new(),
            camera: None,
            bonds: Vec::new(),
        }
    }
}

fn shape_of(collider: &crate::components::Collider) -> Shape {
    use crate::components::Collider;
    match collider {
        Collider::Box { dims, .. } => Shape::Box { dims: *dims },
        Collider::Sphere { radius, .. } => Shape::Sphere { radius: *radius },
        Collider::ConvexHull { points } => Shape::Hull {
            points: points.clone(),
        },
        Collider::Compound(parts) => parts.first().map(shape_of).unwrap_or(Shape::Point),
        Collider::Heightfield(_) => Shape::Point,
    }
}

/// Captures the current `Scene` into a presentation frame. Entities whose ids
/// are in `channels` are reported as channel values; everything else is a
/// visible body (position from `Transform`, or from `state[0..2]` for
/// state-only bodies such as `nbody` particles).
pub fn snapshot_with(
    names: &std::collections::BTreeMap<u128, String>,
    channels: &[u128],
    scene: &Scene,
    camera: Option<CameraVisual>,
) -> PresentationFrame {
    let mut entities = Vec::new();
    let mut channel_list = Vec::new();
    for (id, e) in &scene.entities {
        // RFC-0038: inactive pool slots are not part of the render view.
        if !e.active {
            continue;
        }
        let shape = match &e.collider {
            Some(c) => shape_of(c),
            None => Shape::Point,
        };
        let state = e
            .state
            .as_ref()
            .map(|s| s.values.clone())
            .unwrap_or_default();
        // An entity whose id is a declared channel reports its mailbox value.
        if channels.contains(&id.0) && e.state.is_some() {
            channel_list.push(ChannelVisual {
                id: id.0,
                value: state.first().copied().unwrap_or(0.0),
            });
            continue;
        }
        // Position: from Transform, or from state slots for state-only bodies.
        let position = match e.transform.map(|t| t.position) {
            Some(p) => p,
            None => Vec3::new(
                state.first().copied().unwrap_or(0.0),
                state.get(1).copied().unwrap_or(0.0),
                state.get(2).copied().unwrap_or(0.0),
            ),
        };
        let rotation = match e.transform.map(|t| t.rotation) {
            Some(q) => q,
            None => {
                let orient = e.render.as_ref().map(|r| r.orient).unwrap_or(false);
                if orient {
                    // `orient = true`: state[7/8/9] are euler (pitch, yaw, roll) —
                    // the body faces and leans as the simulation steers it.
                    Quat::from_yaw_pitch_roll(
                        state.get(8).copied().unwrap_or(0.0),
                        state.get(7).copied().unwrap_or(0.0),
                        state.get(9).copied().unwrap_or(0.0),
                    )
                } else {
                    // Self-rotation (自转): state[7] is a spin angle about Z.
                    let ang = state.get(7).copied().unwrap_or(0.0);
                    Quat {
                        x: 0.0,
                        y: 0.0,
                        z: (ang * 0.5).sin(),
                        w: (ang * 0.5).cos(),
                    }
                }
            }
        };
        let mut vis = EntityVisual {
            id: id.0,
            name: names
                .get(&id.0)
                .cloned()
                .unwrap_or_else(|| format!("#{}", id.0)),
            position,
            rotation,
            shape,
            state: state.clone(),
            color: e.color.unwrap_or_else(|| EntityVisual::color_for(id.0)),
            size: visual_size(&state),
            opacity: 1.0,
            glow: 0.8,
            label: true,
            no_velocity: false,
            style: None,
        };
        // Presentation-only overrides declared in the language.
        if let Some(r) = &e.render {
            if let Some(parts) = &r.parts {
                if !parts.is_empty() {
                    // The entity's `size` scales the whole custom shape (its
                    // amplitude); each part also has its own `scale`.
                    let whole = if r.size.unwrap_or(0.0) > 0.0 {
                        r.size.unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    let group: Vec<Part> = parts
                        .iter()
                        .map(|p| {
                            let base = if p.scale == 0.0 { 1.0 } else { p.scale };
                            let sc = base * whole;
                            let sp = |x: f64, y: f64, z: f64| Vec3::new(x * sc, y * sc, z * sc);
                            let shape = match p.kind {
                                1 => Shape::Sphere { radius: p.a * sc },
                                2 => Shape::Box {
                                    dims: sp(p.a, p.b, p.c),
                                },
                                3 => Shape::Capsule {
                                    radius_bottom: p.a * sc,
                                    length: p.b * sc,
                                    radius_top: p.c * sc,
                                },
                                4 => Shape::Svg {
                                    path: p.path.clone().unwrap_or_default(),
                                    depth: p.a,
                                    scale: sc,
                                },
                                5 => Shape::Hull {
                                    points: p
                                        .points
                                        .iter()
                                        .map(|(x, y, z)| sp(*x, *y, *z))
                                        .collect(),
                                },
                                6 => Shape::Poly {
                                    points: p
                                        .points
                                        .iter()
                                        .map(|(x, y, z)| sp(*x, *y, *z))
                                        .collect(),
                                    faces: p.faces.clone(),
                                },
                                8 => Shape::Ring {
                                    radius: p.a * sc,
                                    tube: p.b * sc,
                                },
                                9 => Shape::Cylinder {
                                    radius: p.a * sc,
                                    height: (if p.b > 0.0 { p.b } else { p.a }) * sc,
                                },
                                10 => Shape::Cone {
                                    radius: p.a * sc,
                                    height: (if p.b > 0.0 { p.b } else { p.a }) * sc,
                                },
                                11 => Shape::Plane {
                                    width: p.a * sc,
                                    height: (if p.b > 0.0 { p.b } else { p.a }) * sc,
                                },
                                _ => Shape::Point,
                            };
                            Part {
                                shape,
                                offset: Vec3::new(
                                    p.offset.0 * whole,
                                    p.offset.1 * whole,
                                    p.offset.2 * whole,
                                ),
                                color: p.color,
                                opacity: p.opacity,
                                orbit: p.orbit.map(|(radius, speed, phase)| Orbit {
                                    radius: radius * whole,
                                    speed,
                                    phase,
                                    axis: p
                                        .orbit_axis
                                        .map(|(x, y, z)| Vec3::new(x, y, z))
                                        .unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
                                }),
                                spin: p.spin,
                            }
                        })
                        .collect();
                    vis.shape = Shape::Group(group);
                }
            }
            if let Some(code) = r.shape {
                vis.shape = match code {
                    1 => Shape::Sphere {
                        radius: r.size.unwrap_or(0.3),
                    },
                    2 => {
                        let dims = match r.size3 {
                            Some((x, y, z)) => Vec3::new(x, y, z),
                            None => {
                                let e = r.size.unwrap_or(0.5);
                                Vec3::new(e, e, e)
                            }
                        };
                        Shape::Box { dims }
                    }
                    3 => {
                        let (r0, len, r1) = r.size3.unwrap_or((
                            r.size.unwrap_or(0.06),
                            0.3,
                            r.size.unwrap_or(0.06),
                        ));
                        Shape::Capsule {
                            radius_bottom: r0,
                            length: len,
                            radius_top: r1,
                        }
                    }
                    _ => Shape::Point,
                };
            }
            if let Some(sz) = r.size {
                vis.size = sz.max(0.01);
            }
            if let Some(o) = r.opacity {
                vis.opacity = o.clamp(0.0, 1.0);
            }
            if let Some(g) = r.glow {
                vis.glow = g.max(0.0);
            }
            if let Some(l) = r.label {
                vis.label = l;
            }
            vis.no_velocity = r.no_velocity;
        }
        entities.push(vis);
    }
    entities.sort_by_key(|e| e.id);
    channel_list.sort_by_key(|c| c.id);
    // Fields: sampled cell values in linear order, capped by a stride.
    let mut fields = Vec::new();
    for (name, f) in &scene.fields {
        let total = f.cells().len();
        let stride = if total > MAX_FIELD_POINTS {
            total.div_ceil(MAX_FIELD_POINTS)
        } else {
            1
        };
        let cells = f.cells().iter().step_by(stride).copied().collect();
        fields.push(FieldVisual {
            name: name.clone(),
            width: f.width,
            height: f.height,
            depth: f.depth,
            dx: f.dx,
            stride,
            cells,
        });
    }
    PresentationFrame {
        time: scene.sim_time,
        entities,
        channels: channel_list,
        fields,
        camera,
        bonds: Vec::new(),
    }
}

/// Returns a copy of `frame` with the given bonded entity-id pairs attached
/// (for molecule rendering).
pub fn with_bonds(frame: &PresentationFrame, bonds: &[Bond]) -> PresentationFrame {
    let mut out = frame.clone();
    out.bonds = bonds.to_vec();
    out
}

/// Captures a scene with no channels (all state-only entities are bodies).
pub fn snapshot(scene: &Scene, camera: Option<CameraVisual>) -> PresentationFrame {
    snapshot_with(&Default::default(), &[], scene, camera)
}

fn fmt_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".to_string()
    }
}

/// The optional `vis` material block: only the set fields are emitted, so the
/// viewer falls back to its defaults for everything else.
fn style_json(st: &VisualStyle) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = st.metalness {
        parts.push(format!("\"metalness\":{}", fmt_f64(v)));
    }
    if let Some(v) = st.roughness {
        parts.push(format!("\"roughness\":{}", fmt_f64(v)));
    }
    if let Some(v) = st.emissive {
        parts.push(format!("\"emissive\":{}", v & 0xFF_FFFF));
    }
    if let Some(v) = st.emissive_intensity {
        parts.push(format!("\"emissiveIntensity\":{}", fmt_f64(v)));
    }
    if let Some(v) = st.wireframe {
        parts.push(format!("\"wireframe\":{v}"));
    }
    if let Some(v) = st.flat {
        parts.push(format!("\"flat\":{v}"));
    }
    if let Some(v) = st.double_sided {
        parts.push(format!("\"doubleSided\":{v}"));
    }
    if let Some(v) = st.cast_shadow {
        parts.push(format!("\"castShadow\":{v}"));
    }
    format!("{{{}}}", parts.join(","))
}

fn vec3_json(v: Vec3) -> String {
    format!("[{},{},{}]", fmt_f64(v.x), fmt_f64(v.y), fmt_f64(v.z))
}

fn quat_json(q: Quat) -> String {
    format!(
        "[{},{},{},{}]",
        fmt_f64(q.x),
        fmt_f64(q.y),
        fmt_f64(q.z),
        fmt_f64(q.w)
    )
}

/// Escapes `s` as a JSON string body (no surrounding quotes).
///
/// Beyond the JSON minimum (backslash, quote, control characters), the angle
/// brackets and the U+2028/U+2029 line separators are escaped as well: frames
/// are embedded in a `<script>` element of the generated viewer, so a
/// `</script>` inside a name, SVG path or field name must never close that
/// element, and a line separator must not split the script source.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Serializes a float slice as a JSON array string (`[1,2,3]`); non-finite
/// values become `0`, matching [`fmt_f64`].
fn f64_list_json(vs: &[f64]) -> String {
    let mut out = String::from("[");
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&fmt_f64(*v));
    }
    out.push(']');
    out
}

/// Serializes one frame to a JSON object string (no external serde).
pub fn frame_to_json(frame: &PresentationFrame) -> String {
    let mut out = String::new();
    let _ = write!(out, "{{\"time\":{},\"entities\":[", fmt_f64(frame.time));
    for (i, e) in frame.entities.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{},\"name\":\"{}\",\"pos\":{},\"rot\":{},\"color\":{},\"opacity\":{},\"glow\":{},\"label\":{},\"vec\":{}",
            e.id,
            json_str(&e.name),
            vec3_json(e.position),
            quat_json(e.rotation),
            e.color_value(),
            fmt_f64(e.opacity),
            fmt_f64(e.glow),
            e.label,
            !e.no_velocity
        ));
        if let Some(st) = &e.style {
            out.push_str(",\"vis\":");
            out.push_str(&style_json(st));
        }
        out.push_str(",\"kind\":");
        match &e.shape {
            Shape::Box { dims } => {
                out.push_str(&format!("\"box\",\"dims\":{}}}", vec3_json(*dims)));
            }
            Shape::Sphere { radius } => {
                out.push_str(&format!("\"sphere\",\"radius\":{}}}", fmt_f64(*radius)));
            }
            Shape::Capsule {
                radius_bottom,
                length,
                radius_top,
            } => {
                out.push_str(&format!(
                    "\"capsule\",\"r0\":{},\"len\":{},\"r1\":{}}}",
                    fmt_f64(*radius_bottom),
                    fmt_f64(*length),
                    fmt_f64(*radius_top)
                ));
            }
            Shape::Svg { path, depth, scale } => {
                out.push_str(&format!(
                    "\"svg\",\"d\":\"{}\",\"depth\":{},\"scale\":{}}}",
                    json_str(path),
                    fmt_f64(*depth),
                    fmt_f64(*scale)
                ));
            }
            Shape::Ring { radius, tube } => {
                out.push_str(&format!(
                    "\"ring\",\"a\":{},\"b\":{}}}",
                    fmt_f64(*radius),
                    fmt_f64(*tube)
                ));
            }
            Shape::Cylinder { radius, height } => {
                out.push_str(&format!(
                    "\"cylinder\",\"radius\":{},\"height\":{}}}",
                    fmt_f64(*radius),
                    fmt_f64(*height)
                ));
            }
            Shape::Cone { radius, height } => {
                out.push_str(&format!(
                    "\"cone\",\"radius\":{},\"height\":{}}}",
                    fmt_f64(*radius),
                    fmt_f64(*height)
                ));
            }
            Shape::Plane { width, height } => {
                out.push_str(&format!(
                    "\"plane\",\"w\":{},\"h\":{}}}",
                    fmt_f64(*width),
                    fmt_f64(*height)
                ));
            }
            Shape::Group(parts) => {
                out.push_str("\"group\",\"parts\":[");
                for (j, part) in parts.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!("{{\"off\":{}", vec3_json(part.offset)));
                    match &part.shape {
                        Shape::Sphere { radius } => {
                            let _ = write!(out, ",\"k\":1,\"a\":{}", fmt_f64(*radius));
                        }
                        Shape::Box { dims } => {
                            let _ = write!(
                                out,
                                ",\"k\":2,\"a\":{},\"b\":{},\"c\":{}",
                                fmt_f64(dims.x),
                                fmt_f64(dims.y),
                                fmt_f64(dims.z)
                            );
                        }
                        Shape::Capsule {
                            radius_bottom,
                            length,
                            radius_top,
                        } => {
                            let _ = write!(
                                out,
                                ",\"k\":3,\"a\":{},\"b\":{},\"c\":{}",
                                fmt_f64(*radius_bottom),
                                fmt_f64(*length),
                                fmt_f64(*radius_top)
                            );
                        }
                        Shape::Svg { path, depth, scale } => {
                            let _ = write!(
                                out,
                                ",\"k\":4,\"d\":\"{}\",\"depth\":{},\"scale\":{}",
                                json_str(path),
                                fmt_f64(*depth),
                                fmt_f64(*scale)
                            );
                        }
                        Shape::Hull { points } => {
                            out.push_str(",\"k\":5,\"pts\":[");
                            for (k, p) in points.iter().enumerate() {
                                if k > 0 {
                                    out.push(',');
                                }
                                out.push_str(&vec3_json(*p));
                            }
                            out.push(']');
                        }
                        Shape::Poly { points, faces } => {
                            out.push_str(",\"k\":6,\"pts\":[");
                            for (k, p) in points.iter().enumerate() {
                                if k > 0 {
                                    out.push(',');
                                }
                                out.push_str(&vec3_json(*p));
                            }
                            out.push_str("],\"faces\":[");
                            for (k, f) in faces.iter().enumerate() {
                                if k > 0 {
                                    out.push(',');
                                }
                                out.push('[');
                                for (m, idx) in f.iter().enumerate() {
                                    if m > 0 {
                                        out.push(',');
                                    }
                                    let _ = write!(out, "{idx}");
                                }
                                out.push(']');
                            }
                            out.push(']');
                        }
                        Shape::Ring { radius, tube } => {
                            let _ = write!(
                                out,
                                ",\"k\":8,\"a\":{},\"b\":{}",
                                fmt_f64(*radius),
                                fmt_f64(*tube)
                            );
                        }
                        Shape::Cylinder { radius, height } => {
                            let _ = write!(
                                out,
                                ",\"k\":9,\"a\":{},\"b\":{}",
                                fmt_f64(*radius),
                                fmt_f64(*height)
                            );
                        }
                        Shape::Cone { radius, height } => {
                            let _ = write!(
                                out,
                                ",\"k\":10,\"a\":{},\"b\":{}",
                                fmt_f64(*radius),
                                fmt_f64(*height)
                            );
                        }
                        Shape::Plane { width, height } => {
                            let _ = write!(
                                out,
                                ",\"k\":11,\"a\":{},\"b\":{}",
                                fmt_f64(*width),
                                fmt_f64(*height)
                            );
                        }
                        _ => out.push_str(",\"k\":0"),
                    }
                    if let Some(c) = part.color {
                        let _ = write!(out, ",\"color\":{c}");
                    }
                    if let Some(o) = part.opacity {
                        let _ = write!(out, ",\"opacity\":{}", fmt_f64(o));
                    }
                    if let Some(o) = &part.orbit {
                        let _ = write!(
                            out,
                            ",\"orbit\":[{},{},{},{}]",
                            fmt_f64(o.radius),
                            fmt_f64(o.speed),
                            fmt_f64(o.phase),
                            vec3_json(o.axis)
                        );
                    }
                    if part.spin != 0.0 {
                        let _ = write!(out, ",\"spin\":{}", fmt_f64(part.spin));
                    }
                    out.push('}');
                }
                out.push(']');
                out.push_str(",\"state\":");
                out.push_str(&f64_list_json(&e.state));
                out.push('}');
            }
            Shape::Poly { points, faces } => {
                out.push_str("\"poly\",\"pts\":[");
                for (j, p) in points.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&vec3_json(*p));
                }
                out.push_str("],\"faces\":[");
                for (j, f) in faces.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push('[');
                    for (m, idx) in f.iter().enumerate() {
                        if m > 0 {
                            out.push(',');
                        }
                        let _ = write!(out, "{idx}");
                    }
                    out.push(']');
                }
                out.push_str("]}");
            }
            Shape::Hull { points } => {
                out.push_str("\"hull\",\"points\":[");
                for (j, p) in points.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&vec3_json(*p));
                }
                out.push(']');
                out.push_str(",\"state\":");
                out.push_str(&f64_list_json(&e.state));
                out.push('}');
            }
            Shape::Point => {
                out.push_str("\"point\",\"size\":");
                out.push_str(&fmt_f64(e.size));
                out.push_str(",\"state\":");
                out.push_str(&f64_list_json(&e.state));
                out.push('}');
            }
        }
    }
    out.push_str("],\"channels\":[");
    for (i, c) in frame.channels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{},\"value\":{}}}",
            c.id,
            fmt_f64(c.value)
        ));
    }
    out.push_str("],\"camera\":");
    match frame.camera {
        Some(cam) => {
            out.push_str(&format!(
                "{{\"pos\":{},\"target\":{}}}",
                vec3_json(cam.position),
                vec3_json(cam.target)
            ));
        }
        None => out.push_str("null"),
    }
    out.push_str(",\"fields\":[");
    for (i, f) in frame.fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"name\":\"{}\",\"width\":{},\"height\":{},\"depth\":{},\"dx\":{},\"stride\":{},\"cells\":{}",
            json_str(&f.name),
            f.width,
            f.height,
            f.depth,
            fmt_f64(f.dx),
            f.stride,
            f64_list_json(&f.cells)
        );
        out.push('}');
    }
    out.push_str("],\"bonds\":[");
    for (i, bd) in frame.bonds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"a\":{},\"b\":{},\"order\":{},\"polarity\":{},\"cloud\":{}",
            bd.a,
            bd.b,
            bd.order,
            fmt_f64(bd.polarity),
            bd.cloud
        ));
        if let Some(mn) = bd.min {
            let _ = write!(out, ",\"min\":{}", fmt_f64(mn));
        }
        if let Some(mx) = bd.max {
            let _ = write!(out, ",\"max\":{}", fmt_f64(mx));
        }
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// Writes a single self-contained 3D web viewport over `frames` to `path`:
/// three.js is embedded in the file itself (see [`vendor_json`]), so the
/// generated `.html` runs offline — open it directly from disk (`file://`),
/// no server and no network needed.
pub fn write_viewer(path: &str, frames: &[PresentationFrame]) -> std::io::Result<()> {
    let mut json = String::from("[");
    for (i, f) in frames.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&frame_to_json(f));
    }
    json.push(']');
    std::fs::write(path, template(&json))
}

/// The vendored three.js modules as one JSON object, keyed by the bare
/// specifier the viewer imports them under (`three`, `three/addons/…`).
/// `ConvexGeometry`'s relative import of `ConvexHull` is rewritten to its bare
/// specifier: the bootstrap loads modules from blob URLs, which cannot resolve
/// relative paths. Keys are emitted in dependency order (an importee before
/// its importer) because the bootstrap resolves specifiers as it goes.
/// [`json_str`] keeps the payload inert inside the embedding
/// `<script type="application/json">` element.
fn vendor_json() -> String {
    let convex = include_str!("../vendor/three/addons/geometries/ConvexGeometry.js").replace(
        "'../math/ConvexHull.js'",
        "'three/addons/math/ConvexHull.js'",
    );
    // The postprocessing graph imports siblings (`./Pass.js`) and shaders
    // (`../shaders/CopyShader.js`) relatively; blob URLs cannot resolve
    // relative specifiers, so rewrite them to bare `three/addons/...` keys the
    // bootstrap resolves. The live viewer serves the same files over HTTP,
    // where relative imports resolve naturally (see `vendor_file`).
    let rw = |src: &str| {
        src.replace("'./", "'three/addons/postprocessing/")
            .replace("'../shaders/", "'three/addons/shaders/")
    };
    // Emitted importee-first: a module may only be embedded after every
    // module it imports, because the bootstrap resolves specifiers against
    // the URLs built so far.
    let entries: [(&str, String); 15] = [
        (
            "three",
            include_str!("../vendor/three/three.module.js").to_string(),
        ),
        (
            "three/addons/math/ConvexHull.js",
            include_str!("../vendor/three/addons/math/ConvexHull.js").to_string(),
        ),
        ("three/addons/geometries/ConvexGeometry.js", convex),
        (
            "three/addons/controls/OrbitControls.js",
            include_str!("../vendor/three/addons/controls/OrbitControls.js").to_string(),
        ),
        (
            "three/addons/loaders/SVGLoader.js",
            include_str!("../vendor/three/addons/loaders/SVGLoader.js").to_string(),
        ),
        (
            "three/addons/objects/MarchingCubes.js",
            include_str!("../vendor/three/addons/objects/MarchingCubes.js").to_string(),
        ),
        (
            "three/addons/renderers/CSS2DRenderer.js",
            include_str!("../vendor/three/addons/renderers/CSS2DRenderer.js").to_string(),
        ),
        (
            "three/addons/postprocessing/Pass.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/Pass.js"
            )),
        ),
        (
            "three/addons/shaders/CopyShader.js",
            rw(include_str!("../vendor/three/addons/shaders/CopyShader.js")),
        ),
        (
            "three/addons/shaders/LuminosityHighPassShader.js",
            rw(include_str!(
                "../vendor/three/addons/shaders/LuminosityHighPassShader.js"
            )),
        ),
        (
            "three/addons/postprocessing/MaskPass.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/MaskPass.js"
            )),
        ),
        (
            "three/addons/postprocessing/ShaderPass.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/ShaderPass.js"
            )),
        ),
        (
            "three/addons/postprocessing/RenderPass.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/RenderPass.js"
            )),
        ),
        (
            "three/addons/postprocessing/EffectComposer.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/EffectComposer.js"
            )),
        ),
        (
            "three/addons/postprocessing/UnrealBloomPass.js",
            rw(include_str!(
                "../vendor/three/addons/postprocessing/UnrealBloomPass.js"
            )),
        ),
    ];
    let mut out = String::new();
    for (i, (key, src)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "\"{key}\":\"{}\"", json_str(src));
    }
    format!("{{{out}}}")
}

fn template(frames_json: &str) -> String {
    let vendor = vendor_json();
    format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>PWE 3D viewport</title>
<style>body{{margin:0;overflow:hidden;font-family:monospace;background:#0b0e14;color:#cdd6f4}}
#ui{{position:fixed;bottom:0;left:0;right:0;background:#11141c;padding:8px 12px;display:flex;gap:12px;align-items:center;z-index:10;border-top:1px solid #2a3240}}
#panel{{position:fixed;top:8px;right:8px;width:220px;background:#11141c;padding:8px;border:1px solid #2a3240;font-size:12px;z-index:10;max-height:60vh;overflow:auto}}
button{{background:#3a4a6b;border:none;color:#fff;padding:4px 10px;cursor:pointer;border-radius:4px}}
input[type=range]{{flex:1}}label{{color:#89b4fa}}
.lbl{{color:#fff;background:rgba(10,13,20,.6);padding:0 4px;border-radius:3px;font-size:11px;pointer-events:none;white-space:nowrap}}
</style></head>
<body>
<div id="panel"></div>
<div id="view"></div>
<div id="ui">
  <button id="play">▶</button>
  <button id="step">⏭</button>
  <label>t=<span id="time">0</span></label>
  <input type="range" id="slider" min="0" value="0">
  <label>frame <span id="frame">0</span>/<span id="maxf">0</span></label>
  <button id="lbl" title="show / hide all labels">🏷 Labels</button>
</div>
<script type="application/json" id="pwe-vendor">{vendor}</script>
<script>
// Single-file mode: turn the embedded vendored three.js sources into
// same-origin blob module URLs, rewriting each module's import specifiers to
// the URLs already built. No import map, no server, no network — the page
// runs from file:// as well. Order matters: a module may only be resolved
// after the modules it imports (see `vendor_json`), which is why keys are
// emitted importee-first.
window.__pweVendor = (function(){{
  const srcs = JSON.parse(document.getElementById('pwe-vendor').textContent);
  const urls = {{}};
  for (const k of Object.keys(srcs)) {{
    const src = srcs[k].replace(/from '(three[^']*)'/g, function(m, spec){{
      return urls[spec] ? "from '" + urls[spec] + "'" : m;
    }});
    urls[k] = URL.createObjectURL(new Blob([src], {{type:'text/javascript'}}));
  }}
  return urls;
}})();
</script>
<script type="module">
const V = window.__pweVendor;
const [T, OC, CG, SL, MC, C2D, EC, RP, UBP] = await Promise.all([
  import(V['three']),
  import(V['three/addons/controls/OrbitControls.js']),
  import(V['three/addons/geometries/ConvexGeometry.js']),
  import(V['three/addons/loaders/SVGLoader.js']),
  import(V['three/addons/objects/MarchingCubes.js']),
  import(V['three/addons/renderers/CSS2DRenderer.js']),
  import(V['three/addons/postprocessing/EffectComposer.js']),
  import(V['three/addons/postprocessing/RenderPass.js']),
  import(V['three/addons/postprocessing/UnrealBloomPass.js'])
]).catch(function(err){{
  document.body.insertAdjacentHTML('beforeend','<pre style="position:fixed;z-index:99;left:8px;bottom:8px;background:#11141c;color:#f38ba8;padding:8px;border:1px solid #f38ba8">viewer failed to load vendored three.js: '+err+'</pre>');
  throw err;
}});
const THREE = T;
const {{ OrbitControls }} = OC;
const {{ ConvexGeometry }} = CG;
const {{ SVGLoader }} = SL;
const {{ MarchingCubes }} = MC;
const {{ CSS2DRenderer, CSS2DObject }} = C2D;
const {{ EffectComposer }} = EC;
const {{ RenderPass }} = RP;
const {{ UnrealBloomPass }} = UBP;
const FRAMES = {frames_json};
let idx = 0, playing = false;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0b0e14);
const gridHelp = new THREE.GridHelper(20, 20, 0x2a3240, 0x1a2030); scene.add(gridHelp);
const axesHelp = new THREE.AxesHelper(2); scene.add(axesHelp);
// Lit scene: a shadow-casting key light plus dimmer fill (ACES tone mapping
// below lifts the mid-tones, so the fill does not need to be hot).
scene.add(new THREE.AmbientLight(0xffffff, 0.3));
scene.add(new THREE.HemisphereLight(0x9cc4ff, 0x0b0e14, 0.55));
const dl = new THREE.DirectionalLight(0xffffff, 1.6); dl.position.set(8, 14, 10);
dl.castShadow = true;
dl.shadow.mapSize.set(2048, 2048);
dl.shadow.camera.left = -30; dl.shadow.camera.right = 30;
dl.shadow.camera.top = 30; dl.shadow.camera.bottom = -30;
dl.shadow.camera.near = 1; dl.shadow.camera.far = 90;
dl.shadow.bias = -0.0004; dl.shadow.normalBias = 0.02;
scene.add(dl); scene.add(dl.target);
const sunLight = new THREE.PointLight(0xFFD24A, 2, 100); scene.add(sunLight);
const camera = new THREE.PerspectiveCamera(60, innerWidth/innerHeight, 0.01, 1000);
camera.position.set(8, 8, 8);
const renderer = new THREE.WebGLRenderer({{antialias:true}});
renderer.setSize(innerWidth, innerHeight);
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.05;
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFSoftShadowMap;
document.getElementById('view').appendChild(renderer.domElement);
const labelRenderer = new CSS2DRenderer(); labelRenderer.setSize(innerWidth, innerHeight); labelRenderer.domElement.style.position='absolute'; labelRenderer.domElement.style.top='0'; labelRenderer.domElement.style.pointerEvents='none'; document.getElementById('view').appendChild(labelRenderer.domElement);
// Post-processing: scene pass + a gentle bloom so emissive bodies (sun, glow)
// bleed light like real emitters.
const composer = new EffectComposer(renderer);
composer.addPass(new RenderPass(scene, camera));
const bloomPass = new UnrealBloomPass(new THREE.Vector2(innerWidth, innerHeight), 0.35, 0.5, 0.9);
composer.addPass(bloomPass);
const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping=true; controls.dampingFactor=0.08; controls.autoRotate=true; controls.autoRotateSpeed=1.2;
controls.minDistance=0.5; controls.maxDistance=500; controls.screenSpacePanning=true; controls.maxPolarAngle=Math.PI;
let cameraSet=false;
let labelsOn=true;
document.getElementById('lbl').onclick=()=>{{ labelsOn=!labelsOn; const b=document.getElementById('lbl'); b.style.opacity=labelsOn?'1':'0.45'; if(FRAMES.length) applyFrame(FRAMES[idx]); }};
let userMoved=false; controls.addEventListener('start',()=>{{userMoved=true;controls.autoRotate=false;}});
renderer.domElement.addEventListener('pointerdown',()=>{{controls.autoRotate=false;userMoved=true;}});
(function(){{
  const N=1200;const pos=new Float32Array(N*3);
  for(let i=0;i<N;i++){{const r=60+Math.random()*120;const t=Math.random()*Math.PI*2;const p=Math.acos(2*Math.random()-1);pos[i*3]=r*Math.sin(p)*Math.cos(t);pos[i*3+1]=r*Math.sin(p)*Math.sin(t);pos[i*3+2]=r*Math.cos(p);}}
  const g=new THREE.BufferGeometry();g.setAttribute('position',new THREE.BufferAttribute(pos,3));
  scene.add(new THREE.Points(g,new THREE.PointsMaterial({{color:0xffffff,size:0.3,sizeAttenuation:false}})));
}})();
const raycaster=new THREE.Raycaster();const pointer=new THREE.Vector2();
// Selection feedback: a wireframe box fitted to the selection's world bounds
// (a fixed-size sphere gets depth-occluded inside larger meshes).
const selBox=new THREE.Box3();
const highlight=new THREE.Box3Helper(selBox,0xffffff);highlight.visible=false;scene.add(highlight);
let selectedId=null;
renderer.domElement.addEventListener('pointerdown',(ev)=>{{
  const rect=renderer.domElement.getBoundingClientRect();
  pointer.x=((ev.clientX-rect.left)/rect.width)*2-1; pointer.y=-((ev.clientY-rect.top)/rect.height)*2+1;
  raycaster.setFromCamera(pointer,camera);
  const hits=raycaster.intersectObjects([...meshes.values()], true);
  selectedId=null;
  if(hits.length){{
    // A composite (`group`) entity is hit at a child part: walk up to the
    // mesh carrying the entity (`userData.id`), and keep only the id — meshes
    // are rebuilt every frame, so a held object reference goes stale.
    let o=hits[0].object;
    while(o && (o.userData==null || o.userData.id==null)) o=o.parent;
    if(o) selectedId=o.userData.id;
  }}
  updateSelection();
}});
const panel = document.getElementById('panel');
const meshes = new Map();
// Selection persists across frame rebuilds (by id, not by mesh reference):
// re-resolve it from the current meshes and refresh the highlight right away,
// so a click responds even while playback is paused.
function updateSelection(){{
  const selM = selectedId!=null && meshes.has(selectedId) ? meshes.get(selectedId) : null;
  if (selM) {{ selBox.setFromObject(selM); selBox.expandByScalar(0.03); highlight.visible=true; }}
  else highlight.visible=false;
}}
// Names / field names / info lines are user-authored text pasted into
// innerHTML: escape them so a name like `<img onerror=…>` stays text.
const esc = function(s){{ return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;'); }};
function svgGeo(d, depth, scale){{ const data=new SVGLoader().parse(d); let sh=[]; for(const p of data.paths) sh=sh.concat(SVGLoader.createShapes(p));
  const geo=new THREE.ExtrudeGeometry(sh,{{depth:Math.max(depth,0.001),bevelEnabled:false,curveSegments:16}}); geo.scale(scale,-scale,scale); geo.center(); return geo; }}
function polyGeo(pts, faces){{ const pos=[]; const F=faces&&faces.length?faces:null;
  if(F){{ for(const f of F){{ for(let i=1;i+1<f.length;i++){{ for(const q of [f[0],f[i],f[i+1]]){{ const p=pts[q]; pos.push(p[0],p[1],p[2]); }} }} }} }}
  const g=new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3)); g.computeVertexNormals(); return g; }}
function capsuleGeo(r0,len,r1){{ const cs=8, pts=[];
  for(let i=0;i<=cs;i++){{const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r0*Math.sin(a), -len/2 - r0*Math.cos(a)));}}
  for(let i=0;i<=cs;i++){{const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r1*Math.cos(a), len/2 + r1*Math.sin(a)));}}
  return new THREE.LatheGeometry(pts,28); }}
// Data-driven primitive registry: `kind` resolves through this table, so a
// new frame primitive only needs one entry here — unknown kinds degrade to a
// point marker instead of breaking the viewer.
const PRIMS = {{
  box: p=>new THREE.BoxGeometry(p.dims[0],p.dims[1],p.dims[2]),
  sphere: p=>new THREE.SphereGeometry(p.radius,24,18),
  ring: p=>new THREE.TorusGeometry(p.a||0.5, p.b||0.05, 12, 48),
  capsule: p=>capsuleGeo(p.r0||0.06, p.len||0.3, p.r1||0.06),
  cylinder: p=>new THREE.CylinderGeometry(p.radius, p.radius, p.height, 24),
  cone: p=>new THREE.ConeGeometry(p.radius, p.height, 24),
  plane: p=>new THREE.PlaneGeometry(p.w, p.h),
  svg: p=>svgGeo(p.d, p.depth, p.scale),
  hull: p=>{{ const verts=(p.points||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2])); try {{ return new ConvexGeometry(verts); }} catch(err) {{ return new THREE.SphereGeometry(0.1,8,6); }} }},
  poly: p=>polyGeo(p.pts||[], p.faces||[]),
  point: p=>new THREE.SphereGeometry((p.size||0.25)/2,16,12)
}};
function makeMesh(e, t) {{
  t = t||0;
  const opacity = e.opacity==null?1:e.opacity, glow = e.glow==null?0.8:e.glow;
  const st = e.vis || {{}};
  const matFor = (col, op, ds)=>{{ const c = (col==null? e.color : col), o = (op==null? opacity : op);
    return new THREE.MeshStandardMaterial({{color:c,
      emissive:new THREE.Color(st.emissive==null?c:st.emissive),
      emissiveIntensity:st.emissiveIntensity==null?glow:st.emissiveIntensity,
      metalness:st.metalness==null?0.1:st.metalness,
      roughness:st.roughness==null?0.55:st.roughness,
      transparent:o<1, opacity:o,
      side:(ds || st.double_sided || e.kind==='plane')?THREE.DoubleSide:THREE.FrontSide,
      wireframe:!!st.wireframe, flatShading:!!st.flat}}); }};
  if (e.kind==='group' && e.parts) return groupMesh(e.parts, t, matFor);
  const build = PRIMS[e.kind];
  const geo = build ? build(e) : new THREE.SphereGeometry((e.size||0.25)/2,16,12);
  return new THREE.Mesh(geo, matFor(null));
}}
function groupMesh(parts, t, matFor) {{
  const g=new THREE.Group();
  for (const p of parts) {{ const m=matFor(p.color, p.opacity); let ch;
    if (p.k===2) {{ ch=new THREE.Mesh(new THREE.BoxGeometry(p.a,p.b,p.c), m); }}
    else if (p.k===3) {{ ch=new THREE.Mesh(capsuleGeo(p.a,p.b,p.c), m); }}
    else if (p.k===4) {{ ch=new THREE.Mesh(svgGeo(p.d, p.depth, p.scale), m); }}
    else if (p.k===5) {{ ch=new THREE.Mesh(new ConvexGeometry((p.pts||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2]))), m); }}
    else if (p.k===6) {{ ch=new THREE.Mesh(polyGeo(p.pts||[], p.faces||[]), matFor(p.color, p.opacity, true)); }}
    else if (p.k===8) {{ ch=new THREE.Mesh(new THREE.TorusGeometry(p.a,p.b,12,48), m); }}
    else if (p.k===9) {{ ch=new THREE.Mesh(new THREE.CylinderGeometry(p.a,p.a,p.b,24), m); }}
    else if (p.k===10) {{ ch=new THREE.Mesh(new THREE.ConeGeometry(p.a,p.b,24), m); }}
    else if (p.k===11) {{ ch=new THREE.Mesh(new THREE.PlaneGeometry(p.a,p.b), matFor(p.color, p.opacity, true)); }}
    else {{ ch=new THREE.Mesh(new THREE.SphereGeometry(p.a,20,16), m); }}
    const off=p.off||[0,0,0];
    if (p.orbit) {{ const o=p.orbit, ang=(o[2]||0)+(o[1]||0)*t;
      const ax=new THREE.Vector3(o[3][0],o[3][1],o[3][2]); if(ax.lengthSq()<1e-12) ax.set(0,0,1); ax.normalize();
      const u=new THREE.Vector3(1,0,0); if(Math.abs(ax.dot(u))>0.9) u.set(0,1,0);
      const e1=u.clone().addScaledVector(ax,-ax.dot(u)).normalize();
      const e2=new THREE.Vector3().crossVectors(ax,e1);
      const pos=e1.multiplyScalar(o[0]*Math.cos(ang)).addScaledVector(e2,o[0]*Math.sin(ang));
      ch.position.set(off[0]+pos.x, off[1]+pos.y, off[2]+pos.z);
    }} else ch.position.set(off[0],off[1],off[2]);
    if (p.spin) ch.rotation.y = p.spin*t;
    g.add(ch);
  }}
  return g;
}}
function addBonds(frame){{
  const up=new THREE.Vector3(0,1,0);
  for(const bd of frame.bonds){{
    const A=meshes.get(bd.a),B=meshes.get(bd.b); if(!A||!B) continue;
    const p1=A.position,p2=B.position;
    const dir=p2.clone().sub(p1); const len=dir.length(); if(len<1e-6) continue;
    if(bd.max!=null && len>bd.max) continue; if(bd.min!=null && len<bd.min) continue;
    const n=dir.clone().normalize();
    const ref=Math.abs(n.dot(up))>0.9? new THREE.Vector3(1,0,0): up;
    const v=new THREE.Vector3().crossVectors(n,ref.clone().addScaledVector(n,-n.dot(ref)).normalize());
    const order=Math.max(1,bd.order||1), pol=bd.polarity||0;
    for(let i=0;i<order;i++){{
      const off=(i-(order-1)/2)*0.11;
      const m=new THREE.Mesh(new THREE.CylinderGeometry(0.05,0.05,len,8,1,true), new THREE.MeshStandardMaterial({{color:0xcccccc,metalness:0.3,roughness:0.4,transparent:true,opacity:0.9}}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5).addScaledVector(v,off);
      m.quaternion.setFromUnitVectors(up,n); scene.add(m);decals.push(m);
    }}
    if(pol>0){{
      const ca=new THREE.Color(A.material.color), cb=new THREE.Color(B.material.color);
      const m=new THREE.Mesh(new THREE.CylinderGeometry(0.058,0.058,len,8,1,true), new THREE.MeshStandardMaterial({{color:ca.lerp(cb,0.5+0.5*pol),metalness:0.2,roughness:0.5,transparent:true,opacity:0.55}}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5);
      m.quaternion.setFromUnitVectors(up,n); scene.add(m);decals.push(m);
    }}
    if(bd.cloud){{
      const geo=new THREE.SphereGeometry(1,16,12); geo.scale(0.16,0.16,len*0.7);
      const m=new THREE.Mesh(geo,new THREE.MeshStandardMaterial({{color:0x66ccff,metalness:0.0,roughness:0.3,transparent:true,opacity:0.28}}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0,0,1),n); scene.add(m);decals.push(m);
    }}
  }}
}}
const decals = [];
function addOrbit(center, r, color) {{
  const pts=[]; const N=64;
  for (let i=0;i<=N;i++) {{ const a=i/N*Math.PI*2; pts.push(new THREE.Vector3(center.x+Math.cos(a)*r,center.y+Math.sin(a)*r,center.z)); }}
  const g=new THREE.BufferGeometry().setFromPoints(pts);
  const l=new THREE.Line(g,new THREE.LineBasicMaterial({{color:color,opacity:0.25,transparent:true}}));
  scene.add(l); decals.push(l);
}}
function addVel(x,y,z,vx,vy,vz,color) {{
  const d=new THREE.Vector3(vx,vy,vz); if(d.length()<1e-9) return;
  const dir=d.clone().normalize();
  const a=new THREE.ArrowHelper(dir,new THREE.Vector3(x,y,z),0.6,0xffffff,0.22,0.14);
  scene.add(a); decals.push(a);
}}
const fieldObjects = new Map();
let fieldExtent = 0;
function disposeMat(m){{ if(!m) return; const arr=Array.isArray(m)?m:[m]; for(const mm of arr){{ try{{ if(mm){{ const tex=mm.map; if(tex&&tex.dispose)tex.dispose(); if(mm.dispose)mm.dispose(); }} }}catch(e){{}} }} }}
function disposeObj(o){{ if(!o) return; try{{ if(o.geometry&&o.geometry.dispose)o.geometry.dispose(); }}catch(e){{}} disposeMat(o.material); try{{ if(o.element&&o.element.remove)o.element.remove(); }}catch(e){{}} if(o.children) for(const c of o.children) disposeObj(c); }}
// One translucent shell per field, coloured by radius (energy ~ 1/r^2): hot near
// the source -> cool far away, across the shell's own hue family.
function fieldRes(W, H, D) {{ return Math.max(16, Math.min(40, Math.round(1.5*Math.max(W,H,D)))); }}
function makeShell(hot, cool, W, H, D, dx) {{
  const mat = new THREE.MeshStandardMaterial({{vertexColors:true,color:0xffffff,metalness:0.05,roughness:0.5,transparent:true,opacity:0.72,side:THREE.DoubleSide,emissive:0x06101f,emissiveIntensity:0.12}});
  const mc = new MarchingCubes(fieldRes(W,H,D), mat, false, false, 60000);
  mc.scale.set(W*dx/2, H*dx/2, D*dx/2);
  mc.matrixAutoUpdate = false; mc.updateMatrix();
  scene.add(mc);
  return {{mc, mat, hot, cool}};
}}
function colorShell(sh) {{
  const g = sh.mc.geometry, pos = g.getAttribute('position');
  let ca = g.getAttribute('color');
  if (!ca || ca.count !== pos.count) {{ ca = new THREE.BufferAttribute(new Float32Array(pos.count*3), 3); g.setAttribute('color', ca); }}
  const nv = Math.min(pos.count, sh.mc.count), root = Math.sqrt(3);
  for (let i=0; i<nv; i++) {{
    const x=pos.getX(i), y=pos.getY(i), z=pos.getZ(i);
    const t=Math.min(1, Math.sqrt(x*x+y*y+z*z)/root);
    ca.setXYZ(i, sh.hot[0]+(sh.cool[0]-sh.hot[0])*t, sh.hot[1]+(sh.cool[1]-sh.hot[1])*t, sh.hot[2]+(sh.cool[2]-sh.hot[2])*t);
  }}
  ca.needsUpdate = true;
}}
// Render the field. A 1-D field (height = depth = 1) is drawn as a coloured
// oscillating line (y = value): a sine standing wave reads as a sine curve.
// A 2-D/3-D field is drawn as two nested isosurfaces (warm crest + cool trough)
// whose colour also falls off with radius (energy).
function renderFields(fields) {{
  if (!fields) return '';
  const seen = new Set(); let txt = '';
  fields.forEach((fl) => {{
    seen.add(fl.name);
    const W=fl.width, H=fl.height, D=fl.depth, dx=fl.dx, st=fl.stride||1;
    let lo=Infinity, hi=-Infinity;
    for (const v of fl.cells) {{ if (v<lo) lo=v; if (v>hi) hi=v; }}
    if (hi-lo < 1e-6) {{ const old=fieldObjects.get(fl.name);
      if (old) {{ if (old.mc) {{ scene.remove(old.mc); disposeObj(old.mc); if(old.trough&&old.trough.mc) scene.remove(old.trough.mc); }} if (old.line) scene.remove(old.line); if (old.pts) scene.remove(old.pts); fieldObjects.delete(fl.name); }}
      return; }}
    const full=new Float32Array(W*H*D);
    if (st===1) {{ for (let i=0;i<fl.cells.length && i<full.length;i++) full[i]=fl.cells[i]; }}
    else {{ full.fill(0); for (let s=0;s<fl.cells.length;s++) {{ const lin=s*st; if (lin<full.length) full[lin]=fl.cells[s]; }} }}

    if (H<=1 && D<=1) {{
      let ent = fieldObjects.get(fl.name);
      if (!ent || ent.W!==W) {{
        if (ent) {{ if (ent.mc) {{ scene.remove(ent.mc); scene.remove(ent.trough.mc); }} if (ent.line) scene.remove(ent.line); }}
        const geo = new THREE.BufferGeometry();
        geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array(W*3), 3));
        geo.setAttribute('color', new THREE.BufferAttribute(new Float32Array(W*3), 3));
        const line = new THREE.Line(geo, new THREE.LineBasicMaterial({{vertexColors:true}}));
        line.frustumCulled = false; scene.add(line);
        const pts = new THREE.Points(geo, new THREE.PointsMaterial({{vertexColors:true,size:Math.max(dx*0.8,0.5),sizeAttenuation:true}}));
        pts.frustumCulled = false; scene.add(pts);
        ent = {{W, H, D, line, pts}};
        fieldObjects.set(fl.name, ent);
      }}
      const scale = 0.28*W*dx;
      const pos = ent.line.geometry.getAttribute('position');
      const col = ent.line.geometry.getAttribute('color');
      const warm=[1.0,0.55,0.15], cool=[0.15,0.60,1.0];
      for (let i=0; i<W; i++) {{
        const v = Math.max(-1.2, Math.min(1.2, full[i]));
        pos.setXYZ(i, (i-(W-1)/2)*dx, v*scale, 0);
        const t = Math.min(1, Math.abs(v));
        const c = v >= 0 ? warm : cool;
        col.setXYZ(i, c[0]*t+0.06, c[1]*t+0.06, c[2]*t+0.06);
      }}
      pos.needsUpdate = true; col.needsUpdate = true;
      fieldExtent = Math.max(fieldExtent, W*dx, scale*2);
      txt += esc(fl.name)+' 1D |u|max='+hi.toFixed(3)+'<br>';
      return;
    }}

    let ent = fieldObjects.get(fl.name);
    if (!ent || ent.W!==W || ent.H!==H || ent.D!==D) {{
      if (ent) {{ if (ent.mc) {{ scene.remove(ent.mc); scene.remove(ent.trough.mc); }} if (ent.line) scene.remove(ent.line); }}
      ent = {{ W, H, D,
        crest:  makeShell([1.0,0.85,0.30], [1.0,0.42,0.10], W, H, D, dx),
        trough: makeShell([0.30,0.85,1.0], [0.10,0.30,0.95], W, H, D, dx) }};
      fieldObjects.set(fl.name, ent);
    }}
    const R=fieldRes(W,H,D);
    const at=(i,j,k)=>full[k*W*H + j*W + i];
    const fld=new Float32Array(R*R*R);
    for (let z=0; z<R; z++) {{
      const fz=(D===1?0:z/(R-1)*(D-1)), k0=Math.floor(fz), k1=Math.min(k0+1,D-1), tz=fz-k0;
      for (let y=0; y<R; y++) {{
        const fy=(H===1?0:y/(R-1)*(H-1)), j0=Math.floor(fy), j1=Math.min(j0+1,H-1), ty=fy-j0;
        for (let x=0; x<R; x++) {{
          const fx=(W===1?0:x/(R-1)*(W-1)), i0=Math.floor(fx), i1=Math.min(i0+1,W-1), tx=fx-i0;
          const c00=at(i0,j0,k0)+(at(i1,j0,k0)-at(i0,j0,k0))*tx;
          const c10=at(i0,j1,k0)+(at(i1,j1,k0)-at(i0,j1,k0))*tx;
          const c01=at(i0,j0,k1)+(at(i1,j0,k1)-at(i0,j0,k1))*tx;
          const c11=at(i0,j1,k1)+(at(i1,j1,k1)-at(i0,j1,k1))*tx;
          const c0=c00+(c10-c00)*ty, c1=c01+(c11-c01)*ty;
          fld[z*R*R + y*R + x]=c0+(c1-c0)*tz;
        }}
      }}
    }}
    for (let p2=0; p2<2; p2++) {{
      const src = fld.slice();
      for (let z=1; z<R-1; z++) for (let y=1; y<R-1; y++) for (let x=1; x<R-1; x++) {{
        const q = z*R*R + y*R + x;
        fld[q] = (src[q]*6 + src[q-1] + src[q+1] + src[q-R] + src[q+R] + src[q-R*R] + src[q+R*R]) / 12;
      }}
    }}
    let flo=Infinity, fhi=-Infinity;
    for (const v of fld) {{ if (v<flo) flo=v; if (v>fhi) fhi=v; }}
    const span=(fhi>flo)?(fhi-flo):1;
    let peak=0; for (const v of fl.cells) {{ const a=Math.abs(v); if (a>peak) peak=a; }}
    const b=Math.max(0.6, Math.min(1.0, 0.6 + 0.5*peak));
    ent.crest.mc.field.set(fld);  ent.crest.mc.isolation  = flo + 0.70*span; ent.crest.mc.update();
    ent.trough.mc.field.set(fld); ent.trough.mc.isolation = flo + 0.30*span; ent.trough.mc.update();
    colorShell(ent.crest); colorShell(ent.trough);
    ent.crest.mat.color.setScalar(b); ent.trough.mat.color.setScalar(b);
    fieldExtent = Math.max(fieldExtent, W*dx, H*dx, D*dx);
    const dk=(D>1?('\u00d7'+D):'');
    txt += esc(fl.name)+' '+W+'\u00d7'+H+dk+'  E\u221d|u|max '+peak.toFixed(3)+'<br>';
  }});
  for (const [name, ent] of [...fieldObjects]) {{
    if (!seen.has(name)) {{ if (ent.mc) {{ scene.remove(ent.mc); disposeObj(ent.mc); if (ent.trough && ent.trough.mc) {{ scene.remove(ent.trough.mc); disposeObj(ent.trough.mc); }} disposeMat(ent.mat); if (ent.trough) disposeMat(ent.trough.mat); }} if (ent.line) {{ scene.remove(ent.line); disposeObj(ent.line); }} if (ent.pts) {{ scene.remove(ent.pts); disposeObj(ent.pts); }} fieldObjects.delete(name); }}
  }}
  return txt;
}}
// Trails: a fading polyline over each moving entity's recent positions. Static
// mode knows every frame upfront, so histories are precomputed once from
// FRAMES (entities that never move are skipped). Sparse by frame index: an
// entity may be absent from some frames.
const TRAIL_N = 48;
const trailHist = new Map();
{{
  const byId = new Map();
  FRAMES.forEach((f, fi)=>{{ for (const e of f.entities) {{
    if (e.vec===false) continue;
    let a = byId.get(e.id); if (!a) {{ a = []; byId.set(e.id, a); }}
    a[fi] = e.pos;
  }} }});
  for (const [id, arr] of byId) {{
    const pts = arr.filter(Boolean);
    let moved = false;
    for (let i=1;i<pts.length;i++) {{ const dx=pts[i][0]-pts[i-1][0], dy=pts[i][1]-pts[i-1][1], dz=pts[i][2]-pts[i-1][2];
      if (dx*dx+dy*dy+dz*dz > 1e-6) {{ moved = true; break; }} }}
    if (moved && pts.length>=2) trailHist.set(id, arr);
  }}
}}
const trailLines = new Map();
function updateTrails(frame, index) {{
  const seen = new Set();
  for (const e of frame.entities) {{
    const hist = trailHist.get(e.id); if (!hist) continue;
    const end = Math.min(hist.length, index+1), start = Math.max(0, end-TRAIL_N);
    const pts = [];
    for (let i=start;i<end;i++) if (hist[i]) pts.push(hist[i]);
    if (pts.length < 2) continue;
    seen.add(e.id);
    const pos = new Float32Array(pts.length*3), col = new Float32Array(pts.length*3);
    const c = new THREE.Color(e.color);
    for (let i=0;i<pts.length;i++) {{
      pos[i*3]=pts[i][0]; pos[i*3+1]=pts[i][1]; pos[i*3+2]=pts[i][2];
      const a = Math.pow(i/(pts.length-1), 1.5);
      col[i*3]=c.r*a; col[i*3+1]=c.g*a; col[i*3+2]=c.b*a;
    }}
    let line = trailLines.get(e.id);
    if (!line) {{
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.BufferAttribute(pos,3));
      g.setAttribute('color', new THREE.BufferAttribute(col,3));
      line = new THREE.Line(g, new THREE.LineBasicMaterial({{vertexColors:true,transparent:true,depthWrite:false}}));
      line.frustumCulled = false;
      scene.add(line); trailLines.set(e.id, line);
    }} else {{
      const g = line.geometry;
      g.setAttribute('position', new THREE.BufferAttribute(pos,3));
      g.setAttribute('color', new THREE.BufferAttribute(col,3));
      g.attributes.position.needsUpdate = true;
      g.attributes.color.needsUpdate = true;
    }}
    line.visible = true;
  }}
  for (const [id, line] of trailLines) if (!seen.has(id)) line.visible = false;
}}
function applyFrame(f) {{
  const isMol = f.bonds && f.bonds.length>0;
  for (const m of meshes.values()) {{ scene.remove(m); disposeObj(m); }}
  meshes.clear();
  for (const m of decals) {{ scene.remove(m); disposeObj(m); }} decals.length=0;
  let html = '<b>t='+f.time.toFixed(3)+'</b><hr>';
  let sun=null;
  for (const e of f.entities) {{ const r=Math.hypot(e.pos[0],e.pos[1]); if(!sun||r<sun.r) sun={{r:r,x:e.pos[0],y:e.pos[1],z:e.pos[2]}}; }}
  if (sun) sunLight.position.set(sun.x,sun.y,sun.z);
  for (const e of f.entities) {{
    const m = makeMesh(e, f.time);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);
    m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if ((e.name==='sun'||(sun&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6)) && m.material) {{ m.material.emissive=new THREE.Color(e.color); if (e.glow==null) m.material.emissiveIntensity=1.2; }}
    m.userData=e; scene.add(m); meshes.set(e.id, m);
    // Shadows: opaque meshes cast and receive; transparent/wireframe ones
    // only receive (a translucent shadow looks wrong), and `vis.castShadow
    // = false` opts an entity out entirely.
    m.traverse(o=>{{ if(o.isMesh && o.material && !Array.isArray(o.material)){{ o.castShadow = !o.material.transparent && !o.material.wireframe && !(e.vis && e.vis.castShadow===false); o.receiveShadow = !o.material.transparent; }} }});
    if (e.name && e.label!==false && labelsOn) {{ const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name; const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l); }}
    if (e.vec!==false && !isMol && sun && e.state && e.state.length>=6 && Math.hypot(e.state[3],e.state[4],e.state[5])>1e-6) addOrbit(sun, Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y), e.color);
    if (e.vec!==false && !isMol && e.state && e.state.length>=6) addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);
    if (e.state && e.state.length) html += esc(e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
  }}
  if (isMol) addBonds(f);
  const hasFields = f.fields && f.fields.length;
  gridHelp.visible = !hasFields; axesHelp.visible = !hasFields;
  if (hasFields) html += '<hr>' + renderFields(f.fields);
  for (const c of f.channels) html += 'ch#'+c.id+' = '+c.value.toFixed(3)+'<br>';
  panel.innerHTML = html;
  if (f.camera && !cameraSet) {{ camera.position.set(f.camera.pos[0],f.camera.pos[1],f.camera.pos[2]); camera.lookAt(f.camera.target[0],f.camera.target[1],f.camera.target[2]); cameraSet=true; }}
  else if (fieldExtent>0 && !cameraSet && !userMoved) {{ const e=fieldExtent*2.0; camera.position.set(e,e*0.8,e); camera.lookAt(0,0,0); controls.target.set(0,0,0); controls.update(); cameraSet=true; }}
  document.getElementById('time').textContent = f.time.toFixed(3);
  document.getElementById('frame').textContent = idx;
  updateTrails(f, idx);
  // Re-resolve the selection from this frame's freshly built meshes.
  updateSelection();
}}
const slider = document.getElementById('slider');
slider.max = Math.max(0, FRAMES.length-1);
document.getElementById('maxf').textContent = Math.max(0, FRAMES.length-1);
function go(i) {{ idx = Math.max(0, Math.min(FRAMES.length-1, i)); slider.value=idx; applyFrame(FRAMES[idx]); }}
document.getElementById('play').onclick = ()=>{{ playing=!playing; document.getElementById('play').textContent=playing?'⏸':'▶'; }};
document.getElementById('step').onclick = ()=>{{ go(idx+1); }};
slider.oninput = ()=>{{ go(+slider.value); }};
document.addEventListener('keydown', e=>{{ if(e.key===' '){{ document.getElementById('play').onclick(); e.preventDefault(); }} }});
renderer.setAnimationLoop(()=>{{
  if (playing && FRAMES.length) {{ go(idx+1); if (idx>=FRAMES.length-1) playing=false; }}
  controls.update(); composer.render(); labelRenderer.render(scene, camera);
}});
addEventListener('resize', ()=>{{ camera.aspect=innerWidth/innerHeight; camera.updateProjectionMatrix(); renderer.setSize(innerWidth,innerHeight); composer.setSize(innerWidth,innerHeight); labelRenderer.setSize(innerWidth,innerHeight); }});
applyFrame(FRAMES[0]);
</script>
</body></html>
"#
    )
}

// ---------------------------------------------------------------------------
// Live runtime interface: the HTML viewer talks to a running runtime over HTTP
// and displays the simulation in real time, rather than replaying static frames.
// ---------------------------------------------------------------------------

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, RwLock};

/// The live state a running runtime publishes for the browser viewer.
#[derive(Clone, Debug, Default)]
pub struct LiveState {
    pub frame: PresentationFrame,
    /// Simulation step counter.
    pub step: u64,
    /// Recent procedure info (e.g. which systems ran, notable values). The
    /// browser shows this so the simulation *procedure* is visible, not just
    /// positions.
    pub info: Vec<String>,
}

/// Serves a live viewer on `127.0.0.1:port` in a background thread. The browser
/// polls `/state` for the current `LiveState` and `/` for the viewer page.
/// `state` is updated by the simulation loop as it runs.
/// RFC-0041: three.js (r160, MIT — see `reference/vendor/three/LICENSE`) and the
/// viewer's addons, embedded and served locally. The viewer must not import
/// three.js from a CDN: an unreachable import map leaves the tab pending (a
/// hang) and makes the page depend on the network.
fn vendor_file(path: &str) -> Option<&'static str> {
    Some(match path {
        "/vendor/three/three.module.js" => include_str!("../vendor/three/three.module.js"),
        "/vendor/three/addons/controls/OrbitControls.js" => {
            include_str!("../vendor/three/addons/controls/OrbitControls.js")
        }
        "/vendor/three/addons/geometries/ConvexGeometry.js" => {
            include_str!("../vendor/three/addons/geometries/ConvexGeometry.js")
        }
        "/vendor/three/addons/loaders/SVGLoader.js" => {
            include_str!("../vendor/three/addons/loaders/SVGLoader.js")
        }
        "/vendor/three/addons/objects/MarchingCubes.js" => {
            include_str!("../vendor/three/addons/objects/MarchingCubes.js")
        }
        "/vendor/three/addons/renderers/CSS2DRenderer.js" => {
            include_str!("../vendor/three/addons/renderers/CSS2DRenderer.js")
        }
        "/vendor/three/addons/math/ConvexHull.js" => {
            include_str!("../vendor/three/addons/math/ConvexHull.js")
        }
        "/vendor/three/addons/postprocessing/Pass.js" => {
            include_str!("../vendor/three/addons/postprocessing/Pass.js")
        }
        "/vendor/three/addons/postprocessing/MaskPass.js" => {
            include_str!("../vendor/three/addons/postprocessing/MaskPass.js")
        }
        "/vendor/three/addons/postprocessing/ShaderPass.js" => {
            include_str!("../vendor/three/addons/postprocessing/ShaderPass.js")
        }
        "/vendor/three/addons/postprocessing/RenderPass.js" => {
            include_str!("../vendor/three/addons/postprocessing/RenderPass.js")
        }
        "/vendor/three/addons/postprocessing/EffectComposer.js" => {
            include_str!("../vendor/three/addons/postprocessing/EffectComposer.js")
        }
        "/vendor/three/addons/postprocessing/UnrealBloomPass.js" => {
            include_str!("../vendor/three/addons/postprocessing/UnrealBloomPass.js")
        }
        "/vendor/three/addons/shaders/CopyShader.js" => {
            include_str!("../vendor/three/addons/shaders/CopyShader.js")
        }
        "/vendor/three/addons/shaders/LuminosityHighPassShader.js" => {
            include_str!("../vendor/three/addons/shaders/LuminosityHighPassShader.js")
        }
        "/vendor/three/LICENSE" => include_str!("../vendor/three/LICENSE"),
        _ => return None,
    })
}

pub fn serve_live(
    state: Arc<RwLock<LiveState>>,
    reset: Arc<std::sync::atomic::AtomicBool>,
    pause: Arc<std::sync::atomic::AtomicBool>,
    port: u16,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let page = live_viewer_html();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            let reset = Arc::clone(&reset);
            let pause = Arc::clone(&pause);
            let page = page.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = handle_connection(&mut stream, &state, &reset, &pause, &page);
            });
        }
    });
    Ok(())
}

fn handle_connection(
    stream: &mut TcpStream,
    state: &Arc<RwLock<LiveState>>,
    reset: &Arc<std::sync::atomic::AtomicBool>,
    pause: &Arc<std::sync::atomic::AtomicBool>,
    page: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_millis(2000)))?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

    let (status, content_type, body) = if path == "/" {
        ("200 OK", "text/html", page.as_bytes().to_vec())
    } else if let Some(asset) = vendor_file(&path) {
        (
            "200 OK",
            "text/javascript; charset=utf-8",
            asset.as_bytes().to_vec(),
        )
    } else if path == "/state" {
        let live = state.read().unwrap_or_else(|e| e.into_inner());
        let body = live_state_json(&live);
        ("200 OK", "application/json", body.into_bytes())
    } else if path == "/reset" {
        // Restart, paused at step 0 so a viewer can step through from the start.
        reset.store(true, std::sync::atomic::Ordering::Relaxed);
        pause.store(true, std::sync::atomic::Ordering::Relaxed);
        ("200 OK", "text/plain", b"ok".to_vec())
    } else if path.starts_with("/pause") {
        let now = if path.contains("on=0") {
            false
        } else if path.contains("on=1") {
            true
        } else {
            !pause.load(std::sync::atomic::Ordering::Relaxed)
        };
        pause.store(now, std::sync::atomic::Ordering::Relaxed);
        (
            "200 OK",
            "text/plain",
            if now {
                b"paused".to_vec()
            } else {
                b"running".to_vec()
            },
        )
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn live_state_json(live: &LiveState) -> String {
    let mut out = String::new();
    out.push_str(&format!("{{\"step\":{},\"frame\":", live.step));
    out.push_str(&frame_to_json(&live.frame));
    out.push_str(",\"info\":[");
    for (i, s) in live.info.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&json_str(s));
        out.push('"');
    }
    out.push_str("]}");
    out
}

/// The viewer page for live mode: no embedded frames; it polls `/state` on an
/// interval and updates the 3D scene, plus a procedure panel fed by `info`.
fn live_viewer_html() -> String {
    r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>PWE live 3D viewport</title>
<style>body{margin:0;overflow:hidden;font-family:monospace;background:#0b0e14;color:#cdd6f4}
#panel{position:fixed;top:8px;right:8px;width:240px;background:#11141c;padding:8px;border:1px solid #2a3240;font-size:12px;z-index:10;max-height:60vh;overflow:auto}
#proc{position:fixed;top:8px;left:8px;width:260px;background:#11141c;padding:8px;border:1px solid #2a3240;font-size:11px;z-index:10;color:#a6e3a1;max-height:50vh;overflow:auto}
#conn{position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);color:#89b4fa}
.lbl{color:#fff;background:rgba(10,13,20,.6);padding:0 4px;border-radius:3px;font-size:11px;pointer-events:none;white-space:nowrap}
</style></head>
<body>
<div id="proc"></div>
<div id="panel"></div>
<div id="conn">connecting…</div>
<button id="rst" style="position:fixed;right:8px;bottom:8px;z-index:11;background:#3a4a6b;border:none;color:#fff;padding:6px 12px;cursor:pointer;border-radius:4px;font-family:monospace">⟳ Restart</button>
<button id="pse" style="position:fixed;right:110px;bottom:8px;z-index:11;background:#3a4a6b;border:none;color:#fff;padding:6px 12px;cursor:pointer;border-radius:4px;font-family:monospace">⏸ Pause</button>
<button id="lbl" style="position:fixed;right:210px;bottom:8px;z-index:11;background:#3a4a6b;border:none;color:#fff;padding:6px 12px;cursor:pointer;border-radius:4px;font-family:monospace">🏷 Labels</button>
<script type="importmap">{"imports":{
  "three":"/vendor/three/three.module.js",
  "three/addons/":"/vendor/three/addons/"
}}</script>
<script type="module">
import * as THREE from 'three';
import {OrbitControls} from 'three/addons/controls/OrbitControls.js';
import {ConvexGeometry} from 'three/addons/geometries/ConvexGeometry.js';
import {SVGLoader} from 'three/addons/loaders/SVGLoader.js';
import {MarchingCubes} from 'three/addons/objects/MarchingCubes.js';
import {CSS2DRenderer,CSS2DObject} from 'three/addons/renderers/CSS2DRenderer.js';
import {EffectComposer} from 'three/addons/postprocessing/EffectComposer.js';
import {RenderPass} from 'three/addons/postprocessing/RenderPass.js';
import {UnrealBloomPass} from 'three/addons/postprocessing/UnrealBloomPass.js';
const scene=new THREE.Scene(); scene.background=new THREE.Color(0x0b0e14);
const gridHelp=new THREE.GridHelper(20,20,0x2a3240,0x1a2030); scene.add(gridHelp); const axesHelp=new THREE.AxesHelper(2); scene.add(axesHelp);
scene.add(new THREE.AmbientLight(0xffffff,0.3)); scene.add(new THREE.HemisphereLight(0x9cc4ff,0x0b0e14,0.55)); const dl=new THREE.DirectionalLight(0xffffff,1.6); dl.position.set(8,14,10); dl.castShadow=true; dl.shadow.mapSize.set(2048,2048); dl.shadow.camera.left=-30; dl.shadow.camera.right=30; dl.shadow.camera.top=30; dl.shadow.camera.bottom=-30; dl.shadow.camera.near=1; dl.shadow.camera.far=90; dl.shadow.bias=-0.0004; dl.shadow.normalBias=0.02; scene.add(dl);
const sunLight=new THREE.PointLight(0xFFD24A,2,100); scene.add(sunLight);
const camera=new THREE.PerspectiveCamera(60,innerWidth/innerHeight,0.01,1000); camera.position.set(8,8,8);
const renderer=new THREE.WebGLRenderer({antialias:true}); renderer.setSize(innerWidth,innerHeight);
renderer.toneMapping=THREE.ACESFilmicToneMapping; renderer.toneMappingExposure=1.05;
renderer.shadowMap.enabled=true; renderer.shadowMap.type=THREE.PCFSoftShadowMap;
document.body.appendChild(renderer.domElement);
const labelRenderer=new CSS2DRenderer(); labelRenderer.setSize(innerWidth,innerHeight); labelRenderer.domElement.style.position='absolute'; labelRenderer.domElement.style.top='0'; labelRenderer.domElement.style.pointerEvents='none'; document.body.appendChild(labelRenderer.domElement);
const composer=new EffectComposer(renderer);
composer.addPass(new RenderPass(scene,camera));
const bloomPass=new UnrealBloomPass(new THREE.Vector2(innerWidth,innerHeight),0.35,0.5,0.9);
composer.addPass(bloomPass);
const controls=new OrbitControls(camera,renderer.domElement);
controls.enableDamping=true; controls.dampingFactor=0.08; controls.autoRotate=true; controls.autoRotateSpeed=1.4;
controls.minDistance=0.5; controls.maxDistance=250; controls.screenSpacePanning=true; controls.maxPolarAngle=Math.PI;
let cameraInit=false;
let labelsOn=true;
let userMoved=false; controls.addEventListener('start',()=>{userMoved=true;controls.autoRotate=false;});
renderer.domElement.addEventListener('pointerdown',()=>{controls.autoRotate=false;userMoved=true;});
// Starfield background.
(function(){const N=1200;const pos=new Float32Array(N*3);for(let i=0;i<N;i++){const r=60+Math.random()*120;const t=Math.random()*Math.PI*2;const p=Math.acos(2*Math.random()-1);pos[i*3]=r*Math.sin(p)*Math.cos(t);pos[i*3+1]=r*Math.sin(p)*Math.sin(t);pos[i*3+2]=r*Math.cos(p);}const g=new THREE.BufferGeometry();g.setAttribute('position',new THREE.BufferAttribute(pos,3));const stars=new THREE.Points(g,new THREE.PointsMaterial({color:0xffffff,size:0.3,sizeAttenuation:false}));scene.add(stars);})();
// Click-to-inspect.
const raycaster=new THREE.Raycaster();const pointer=new THREE.Vector2();
// Selection feedback: a wireframe box fitted to the selection's world bounds
// (a fixed-size sphere gets depth-occluded inside larger meshes).
const selBox=new THREE.Box3();
const highlight=new THREE.Box3Helper(selBox,0xffffff);highlight.visible=false;scene.add(highlight);
let selectedId=null;const inspect=document.createElement('div');inspect.style.cssText='position:fixed;left:8px;bottom:56px;background:#11141c;border:1px solid #2a3240;padding:8px;font-size:11px;z-index:10;max-width:300px;';document.body.appendChild(inspect);
renderer.domElement.addEventListener('pointerdown',(ev)=>{
  const rect=renderer.domElement.getBoundingClientRect();
  pointer.x=((ev.clientX-rect.left)/rect.width)*2-1; pointer.y=-((ev.clientY-rect.top)/rect.height)*2+1;
  raycaster.setFromCamera(pointer,camera);
  const hits=raycaster.intersectObjects([...meshes.values()], true);
  selectedId=null;
  if(hits.length){
    // A composite (`group`) entity is hit at a child part: walk up to the mesh
    // carrying the entity (`userData.id`), and keep only the id — meshes are
    // rebuilt every poll, so a held object reference goes stale.
    let o=hits[0].object;
    while(o && (o.userData==null || o.userData.id==null)) o=o.parent;
    if(o) selectedId=o.userData.id;
  }
  updateSelection();
});
const panel=document.getElementById('panel'), procEl=document.getElementById('proc'), conn=document.getElementById('conn');
const meshes=new Map();
// Selection persists across rebuilds (by id, not by mesh reference):
// re-resolve it from the current meshes and refresh the highlight / inspect
// card right away, so a click responds even between polls.
function updateSelection(){
  const selM=selectedId!=null&&meshes.has(selectedId)?meshes.get(selectedId):null;
  const selE=selM?selM.userData:null;
  if(selM){selBox.setFromObject(selM);selBox.expandByScalar(0.03);highlight.visible=true;} else highlight.visible=false;
  if(selE){
    const st=selE.state||[];
    inspect.innerHTML='<b>'+esc(selE.name)+'</b> ('+esc(selE.kind)+')<br>pos '+selE.pos.map(x=>x.toFixed(2)).join(', ')+'<br>'+(st.length?'state ['+st.map(x=>x.toFixed(3)).join(', ')+']':'')+'<br>size '+ (selE.size||0.25).toFixed(2);
  } else {inspect.innerHTML='';}
}
// Names / field names / info lines are user-authored text pasted into
// innerHTML: escape them so a name like `<img onerror=…>` stays text.
const esc=s=>String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
function svgGeo(d, depth, scale){ const data=new SVGLoader().parse(d); let sh=[]; for(const p of data.paths) sh=sh.concat(SVGLoader.createShapes(p));
  const geo=new THREE.ExtrudeGeometry(sh,{depth:Math.max(depth,0.001),bevelEnabled:false,curveSegments:16}); geo.scale(scale,-scale,scale); geo.center(); return geo; }
function polyGeo(pts, faces){ const pos=[]; const F=faces&&faces.length?faces:null;
  if(F){ for(const f of F){ for(let i=1;i+1<f.length;i++){ for(const q of [f[0],f[i],f[i+1]]){ const p=pts[q]; pos.push(p[0],p[1],p[2]); } } } }
  const g=new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3)); g.computeVertexNormals(); return g; }
function capsuleGeo(r0,len,r1){ const cs=8, pts=[];
  for(let i=0;i<=cs;i++){const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r0*Math.sin(a), -len/2 - r0*Math.cos(a)));}
  for(let i=0;i<=cs;i++){const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r1*Math.cos(a), len/2 + r1*Math.sin(a)));}
  return new THREE.LatheGeometry(pts,28); }
// Data-driven primitive registry: `kind` resolves through this table, so a
// new frame primitive only needs one entry here — unknown kinds degrade to a
// point marker instead of breaking the viewer.
const PRIMS = {
  box: p=>new THREE.BoxGeometry(p.dims[0],p.dims[1],p.dims[2]),
  sphere: p=>new THREE.SphereGeometry(p.radius,24,18),
  ring: p=>new THREE.TorusGeometry(p.a||0.5, p.b||0.05, 12, 48),
  capsule: p=>capsuleGeo(p.r0||0.06, p.len||0.3, p.r1||0.06),
  cylinder: p=>new THREE.CylinderGeometry(p.radius, p.radius, p.height, 24),
  cone: p=>new THREE.ConeGeometry(p.radius, p.height, 24),
  plane: p=>new THREE.PlaneGeometry(p.w, p.h),
  svg: p=>svgGeo(p.d, p.depth, p.scale),
  hull: p=>{ const verts=(p.points||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2])); try { return new ConvexGeometry(verts); } catch(err) { return new THREE.SphereGeometry(0.1,8,6); } },
  poly: p=>polyGeo(p.pts||[], p.faces||[]),
  point: p=>new THREE.SphereGeometry((p.size||0.25)/2,16,12)
};
function make(e, t){
  t = t||0;
  const opacity=e.opacity==null?1:e.opacity, glow=e.glow==null?0.8:e.glow;
  const st = e.vis || {};
  const matFor=(col,op,ds)=>{ const c=(col==null? e.color : col), o=(op==null? opacity : op);
    return new THREE.MeshStandardMaterial({color:c,
      emissive:new THREE.Color(st.emissive==null?c:st.emissive),
      emissiveIntensity:st.emissiveIntensity==null?glow:st.emissiveIntensity,
      metalness:st.metalness==null?0.1:st.metalness,
      roughness:st.roughness==null?0.55:st.roughness,
      transparent:o<1, opacity:o,
      side:(ds || st.double_sided || e.kind==='plane')?THREE.DoubleSide:THREE.FrontSide,
      wireframe:!!st.wireframe, flatShading:!!st.flat}); };
  if(e.kind==='group' && e.parts){
    const g=new THREE.Group();
    for(const p of e.parts){ const m=matFor(p.color, p.opacity); let ch;
      if(p.k===2){ ch=new THREE.Mesh(new THREE.BoxGeometry(p.a,p.b,p.c), m); }
      else if(p.k===3){ ch=new THREE.Mesh(capsuleGeo(p.a,p.b,p.c), m); }
      else if(p.k===4){ ch=new THREE.Mesh(svgGeo(p.d, p.depth, p.scale), m); }
      else if(p.k===5){ ch=new THREE.Mesh(new ConvexGeometry((p.pts||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2]))), m); }
      else if(p.k===6){ ch=new THREE.Mesh(polyGeo(p.pts||[], p.faces||[]), matFor(p.color, p.opacity, true)); }
      else if(p.k===8){ ch=new THREE.Mesh(new THREE.TorusGeometry(p.a,p.b,12,48), m); }
      else if(p.k===9){ ch=new THREE.Mesh(new THREE.CylinderGeometry(p.a,p.a,p.b,24), m); }
      else if(p.k===10){ ch=new THREE.Mesh(new THREE.ConeGeometry(p.a,p.b,24), m); }
      else if(p.k===11){ ch=new THREE.Mesh(new THREE.PlaneGeometry(p.a,p.b), matFor(p.color, p.opacity, true)); }
      else { ch=new THREE.Mesh(new THREE.SphereGeometry(p.a,20,16), m); }
      const off=p.off||[0,0,0];
      if(p.orbit){ const o=p.orbit, ang=(o[2]||0)+(o[1]||0)*t;
        const ax=new THREE.Vector3(o[3][0],o[3][1],o[3][2]); if(ax.lengthSq()<1e-12) ax.set(0,0,1); ax.normalize();
        const u=new THREE.Vector3(1,0,0); if(Math.abs(ax.dot(u))>0.9) u.set(0,1,0);
        const e1=u.clone().addScaledVector(ax,-ax.dot(u)).normalize();
        const e2=new THREE.Vector3().crossVectors(ax,e1);
        const pos=e1.multiplyScalar(o[0]*Math.cos(ang)).addScaledVector(e2,o[0]*Math.sin(ang));
        ch.position.set(off[0]+pos.x, off[1]+pos.y, off[2]+pos.z);
      } else ch.position.set(off[0],off[1],off[2]);
      if(p.spin) ch.rotation.y = p.spin*t;
      g.add(ch);
    }
    return g;
  }
  const build = PRIMS[e.kind];
  const geo = build ? build(e) : new THREE.SphereGeometry((e.size||0.25)/2,16,12);
  return new THREE.Mesh(geo, matFor(null));
}
const fieldObjects = new Map();
let fieldExtent = 0;
function disposeMat(m){ if(!m) return; const arr=Array.isArray(m)?m:[m]; for(const mm of arr){ try{ if(mm){ const tex=mm.map; if(tex&&tex.dispose)tex.dispose(); if(mm.dispose)mm.dispose(); } }catch(e){} } }
function disposeObj(o){ if(!o) return; try{ if(o.geometry&&o.geometry.dispose)o.geometry.dispose(); }catch(e){} disposeMat(o.material); try{ if(o.element&&o.element.remove)o.element.remove(); }catch(e){} if(o.children) for(const c of o.children) disposeObj(c); }
// One translucent shell per field, coloured by radius (energy ~ 1/r^2): hot near
// the source -> cool far away, across the shell's own hue family.
function fieldRes(W, H, D) { return Math.max(16, Math.min(40, Math.round(1.5*Math.max(W,H,D)))); }
function makeShell(hot, cool, W, H, D, dx) {
  const mat = new THREE.MeshStandardMaterial({vertexColors:true,color:0xffffff,metalness:0.05,roughness:0.5,transparent:true,opacity:0.72,side:THREE.DoubleSide,emissive:0x06101f,emissiveIntensity:0.12});
  const mc = new MarchingCubes(fieldRes(W,H,D), mat, false, false, 60000);
  mc.scale.set(W*dx/2, H*dx/2, D*dx/2);
  mc.matrixAutoUpdate = false; mc.updateMatrix();
  scene.add(mc);
  return {mc, mat, hot, cool};
}
function colorShell(sh) {
  const g = sh.mc.geometry, pos = g.getAttribute('position');
  let ca = g.getAttribute('color');
  if (!ca || ca.count !== pos.count) { ca = new THREE.BufferAttribute(new Float32Array(pos.count*3), 3); g.setAttribute('color', ca); }
  const nv = Math.min(pos.count, sh.mc.count), root = Math.sqrt(3);
  for (let i=0; i<nv; i++) {
    const x=pos.getX(i), y=pos.getY(i), z=pos.getZ(i);
    const t=Math.min(1, Math.sqrt(x*x+y*y+z*z)/root);
    ca.setXYZ(i, sh.hot[0]+(sh.cool[0]-sh.hot[0])*t, sh.hot[1]+(sh.cool[1]-sh.hot[1])*t, sh.hot[2]+(sh.cool[2]-sh.hot[2])*t);
  }
  ca.needsUpdate = true;
}
// Render the field. A 1-D field (height = depth = 1) is drawn as a coloured
// oscillating line (y = value): a sine standing wave reads as a sine curve.
// A 2-D/3-D field is drawn as two nested isosurfaces (warm crest + cool trough)
// whose colour also falls off with radius (energy).
function renderFields(fields) {
  if (!fields) return '';
  const seen = new Set(); let txt = '';
  fields.forEach((fl) => {
    seen.add(fl.name);
    const W=fl.width, H=fl.height, D=fl.depth, dx=fl.dx, st=fl.stride||1;
    let lo=Infinity, hi=-Infinity;
    for (const v of fl.cells) { if (v<lo) lo=v; if (v>hi) hi=v; }
    // A uniform field has nothing to draw: remove any stale mesh from an
    // earlier frame (otherwise the isosurface lingers after the wave decays).
    if (hi-lo < 1e-6) { const old=fieldObjects.get(fl.name);
      if (old) { if (old.mc) { scene.remove(old.mc); disposeObj(old.mc); if(old.trough&&old.trough.mc) scene.remove(old.trough.mc); } if (old.line) scene.remove(old.line); if (old.pts) scene.remove(old.pts); fieldObjects.delete(fl.name); }
      return; }
    const full=new Float32Array(W*H*D);
    if (st===1) { for (let i=0;i<fl.cells.length && i<full.length;i++) full[i]=fl.cells[i]; }
    else { full.fill(0); for (let s=0;s<fl.cells.length;s++) { const lin=s*st; if (lin<full.length) full[lin]=fl.cells[s]; } }

    if (H<=1 && D<=1) {
      let ent = fieldObjects.get(fl.name);
      if (!ent || ent.W!==W) {
        if (ent) { if (ent.mc) { scene.remove(ent.mc); scene.remove(ent.trough.mc); } if (ent.line) scene.remove(ent.line); }
        const geo = new THREE.BufferGeometry();
        geo.setAttribute('position', new THREE.BufferAttribute(new Float32Array(W*3), 3));
        geo.setAttribute('color', new THREE.BufferAttribute(new Float32Array(W*3), 3));
        const line = new THREE.Line(geo, new THREE.LineBasicMaterial({vertexColors:true}));
        line.frustumCulled = false; scene.add(line);
        const pts = new THREE.Points(geo, new THREE.PointsMaterial({vertexColors:true,size:Math.max(dx*0.8,0.5),sizeAttenuation:true}));
        pts.frustumCulled = false; scene.add(pts);
        ent = {W, H, D, line, pts};
        fieldObjects.set(fl.name, ent);
      }
      const scale = 0.28*W*dx;
      const pos = ent.line.geometry.getAttribute('position');
      const col = ent.line.geometry.getAttribute('color');
      const warm=[1.0,0.55,0.15], cool=[0.15,0.60,1.0];
      for (let i=0; i<W; i++) {
        const v = Math.max(-1.2, Math.min(1.2, full[i]));
        pos.setXYZ(i, (i-(W-1)/2)*dx, v*scale, 0);
        const t = Math.min(1, Math.abs(v));
        const c = v >= 0 ? warm : cool;
        col.setXYZ(i, c[0]*t+0.06, c[1]*t+0.06, c[2]*t+0.06);
      }
      pos.needsUpdate = true; col.needsUpdate = true;
      fieldExtent = Math.max(fieldExtent, W*dx, scale*2);
      txt += esc(fl.name)+' 1D |u|max='+hi.toFixed(3)+'<br>';
      return;
    }

    let ent = fieldObjects.get(fl.name);
    if (!ent || ent.W!==W || ent.H!==H || ent.D!==D) {
      if (ent) { if (ent.mc) { scene.remove(ent.mc); scene.remove(ent.trough.mc); } if (ent.line) scene.remove(ent.line); }
      ent = { W, H, D,
        crest:  makeShell([1.0,0.85,0.30], [1.0,0.42,0.10], W, H, D, dx),
        trough: makeShell([0.30,0.85,1.0], [0.10,0.30,0.95], W, H, D, dx) };
      fieldObjects.set(fl.name, ent);
    }
    const R=fieldRes(W,H,D);
    const at=(i,j,k)=>full[k*W*H + j*W + i];
    const fld=new Float32Array(R*R*R);
    for (let z=0; z<R; z++) {
      const fz=(D===1?0:z/(R-1)*(D-1)), k0=Math.floor(fz), k1=Math.min(k0+1,D-1), tz=fz-k0;
      for (let y=0; y<R; y++) {
        const fy=(H===1?0:y/(R-1)*(H-1)), j0=Math.floor(fy), j1=Math.min(j0+1,H-1), ty=fy-j0;
        for (let x=0; x<R; x++) {
          const fx=(W===1?0:x/(R-1)*(W-1)), i0=Math.floor(fx), i1=Math.min(i0+1,W-1), tx=fx-i0;
          const c00=at(i0,j0,k0)+(at(i1,j0,k0)-at(i0,j0,k0))*tx;
          const c10=at(i0,j1,k0)+(at(i1,j1,k0)-at(i0,j1,k0))*tx;
          const c01=at(i0,j0,k1)+(at(i1,j0,k1)-at(i0,j0,k1))*tx;
          const c11=at(i0,j1,k1)+(at(i1,j1,k1)-at(i0,j1,k1))*tx;
          const c0=c00+(c10-c00)*ty, c1=c01+(c11-c01)*ty;
          fld[z*R*R + y*R + x]=c0+(c1-c0)*tz;
        }
      }
    }
    for (let p2=0; p2<2; p2++) {
      const src = fld.slice();
      for (let z=1; z<R-1; z++) for (let y=1; y<R-1; y++) for (let x=1; x<R-1; x++) {
        const q = z*R*R + y*R + x;
        fld[q] = (src[q]*6 + src[q-1] + src[q+1] + src[q-R] + src[q+R] + src[q-R*R] + src[q+R*R]) / 12;
      }
    }
    let flo=Infinity, fhi=-Infinity;
    for (const v of fld) { if (v<flo) flo=v; if (v>fhi) fhi=v; }
    const span=(fhi>flo)?(fhi-flo):1;
    let peak=0; for (const v of fl.cells) { const a=Math.abs(v); if (a>peak) peak=a; }
    const b=Math.max(0.6, Math.min(1.0, 0.6 + 0.5*peak));
    ent.crest.mc.field.set(fld);  ent.crest.mc.isolation  = flo + 0.70*span; ent.crest.mc.update();
    ent.trough.mc.field.set(fld); ent.trough.mc.isolation = flo + 0.30*span; ent.trough.mc.update();
    colorShell(ent.crest); colorShell(ent.trough);
    ent.crest.mat.color.setScalar(b); ent.trough.mat.color.setScalar(b);
    fieldExtent = Math.max(fieldExtent, W*dx, H*dx, D*dx);
    const dk=(D>1?('\u00d7'+D):'');
    txt += esc(fl.name)+' '+W+'\u00d7'+H+dk+'  E\u221d|u|max '+peak.toFixed(3)+'<br>';
  });
  for (const [name, ent] of [...fieldObjects]) {
    if (!seen.has(name)) { if (ent.mc) { scene.remove(ent.mc); disposeObj(ent.mc); if (ent.trough && ent.trough.mc) { scene.remove(ent.trough.mc); disposeObj(ent.trough.mc); } disposeMat(ent.mat); if (ent.trough) disposeMat(ent.trough.mat); } if (ent.line) { scene.remove(ent.line); disposeObj(ent.line); } if (ent.pts) { scene.remove(ent.pts); disposeObj(ent.pts); } fieldObjects.delete(name); }
  }
  return txt;
}
// Trails: a fading polyline over each moving entity's recent steps. Each poll
// appends one point when the step advanced and the entity actually moved
// (static entities contribute a single point and never draw a line); a step
// jump backwards (restart/rewind) resets the history.
const TRAIL_N = 48;
const trailHist = new Map();
const trailLines = new Map();
function updateTrails(frame, step){
  const seen = new Set();
  for(const e of frame.entities){
    if(e.vec===false) continue;
    let h = trailHist.get(e.id);
    if(!h){ h = {last:-1, pts:[]}; trailHist.set(e.id, h); }
    if(step > h.last){
      if(h.last >= 0 && step !== h.last + 1) h.pts = [];
      const p = e.pos, q = h.pts[h.pts.length-1];
      if(!q || Math.hypot(p[0]-q[0],p[1]-q[1],p[2]-q[2]) > 1e-6){
        h.pts.push(p);
        if(h.pts.length > TRAIL_N) h.pts.shift();
      }
      h.last = step;
    } else if(step < h.last){
      h.pts = [e.pos]; h.last = step;
    }
    const pts = h.pts;
    if(pts.length < 2) continue;
    seen.add(e.id);
    const pos = new Float32Array(pts.length*3), col = new Float32Array(pts.length*3);
    const c = new THREE.Color(e.color);
    for(let i=0;i<pts.length;i++){
      pos[i*3]=pts[i][0]; pos[i*3+1]=pts[i][1]; pos[i*3+2]=pts[i][2];
      const a = Math.pow(i/(pts.length-1), 1.5);
      col[i*3]=c.r*a; col[i*3+1]=c.g*a; col[i*3+2]=c.b*a;
    }
    let line = trailLines.get(e.id);
    if(!line){
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.BufferAttribute(pos,3));
      g.setAttribute('color', new THREE.BufferAttribute(col,3));
      line = new THREE.Line(g, new THREE.LineBasicMaterial({vertexColors:true,transparent:true,depthWrite:false}));
      line.frustumCulled = false;
      scene.add(line); trailLines.set(e.id, line);
    } else {
      const g = line.geometry;
      g.setAttribute('position', new THREE.BufferAttribute(pos,3));
      g.setAttribute('color', new THREE.BufferAttribute(col,3));
      g.attributes.position.needsUpdate = true;
      g.attributes.color.needsUpdate = true;
    }
    line.visible = true;
  }
  for(const [id, line] of trailLines) if(!seen.has(id)) line.visible = false;
}
function apply(f){
  const isMol = f.frame.bonds && f.frame.bonds.length > 0;
  for(const m of meshes.values()){ scene.remove(m); disposeObj(m); }
  meshes.clear();
  for(const m of decals){ scene.remove(m); disposeObj(m); } decals.length=0;
  let html='<b>step '+f.step+' · t='+f.frame.time.toFixed(3)+'</b><hr>';
  // Sun = the body nearest the origin (central body).
  let sun={x:0,y:0,z:0};
  for(const e of f.frame.entities){const r=Math.hypot(e.pos[0],e.pos[1]);if(!sun.r||r<sun.r){sun.r=r;sun.x=e.pos[0];sun.y=e.pos[1];sun.z=e.pos[2];}}
  sunLight.position.set(sun.x,sun.y,sun.z);
  for(const e of f.frame.entities){
    const m=make(e, f.frame.time);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if((e.name==='sun'||(sun.r&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6))&&m.material){m.material.emissive=new THREE.Color(e.color);if(e.glow==null)m.material.emissiveIntensity=0.6;}
    m.userData=e; scene.add(m);meshes.set(e.id,m);
    // Shadows: opaque meshes cast and receive; transparent/wireframe ones
    // only receive (a translucent shadow looks wrong), and `vis.castShadow
    // = false` opts an entity out entirely.
    m.traverse(o=>{ if(o.isMesh && o.material && !Array.isArray(o.material)){ o.castShadow = !o.material.transparent && !o.material.wireframe && !(e.vis && e.vis.castShadow===false); o.receiveShadow = !o.material.transparent; } });
    // Name label.
    if(e.label!==false && labelsOn){ const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name||('#'+e.id);
    const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l); }
    // For a molecule (bonds present) skip orbit rings; atoms don't orbit.
    if(e.vec!==false && !isMol && e.state && e.state.length>=6 && Math.hypot(e.state[3],e.state[4],e.state[5])>1e-6){ if(sun.r){const r=Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y);addOrbit(sun,r,e.color);} }
    // Velocity vector (skip for static molecule atoms).
    if(e.vec!==false && !isMol && e.state&&e.state.length>=6){addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);}
    html+='<span style="color:#'+e.color.toString(16).padStart(6,'0')+'">■</span> '+esc(e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
  }
  // Draw bonds (molecule) as lines between bonded atoms.
  if(isMol) addBonds(f.frame);
  const hasFields=f.frame.fields&&f.frame.fields.length; gridHelp.visible=!hasFields; axesHelp.visible=!hasFields;
  if(hasFields) html+='<hr>'+renderFields(f.frame.fields);
  for(const c of f.frame.channels) html+='ch#'+c.id+' = '+c.value.toFixed(3)+'<br>';
  panel.innerHTML=html;
  let p=''; for(let i=f.info.length-1;i>=0;i--) p+=esc(f.info[i])+'<br>'; procEl.innerHTML=p;
  if(f.frame.camera&&!cameraInit){camera.position.set(f.frame.camera.pos[0],f.frame.camera.pos[1],f.frame.camera.pos[2]);camera.lookAt(f.frame.camera.target[0],f.frame.camera.target[1],f.frame.camera.target[2]);cameraInit=true;}
  else if(fieldExtent>0&&!cameraInit){const e=fieldExtent*2.0;camera.position.set(e,e*0.8,e);camera.lookAt(0,0,0);controls.target.set(0,0,0);controls.update();cameraInit=true;}
  // Rolling trails over the polled steps.
  updateTrails(f.frame, f.step);
  // Highlight + inspect the selected body, re-resolved from this poll's
  // freshly built meshes (by id; a mesh reference would go stale).
  updateSelection();
}
const decals=[];
function addOrbit(center,r,color){
  const pts=[]; const N=64;
  for(let i=0;i<=N;i++){const a=i/N*Math.PI*2;pts.push(new THREE.Vector3(center.x+Math.cos(a)*r,center.y+Math.sin(a)*r,center.z));}
  const g=new THREE.BufferGeometry().setFromPoints(pts);
  const l=new THREE.Line(g,new THREE.LineBasicMaterial({color:color,opacity:0.25,transparent:true}));
  scene.add(l);decals.push(l);
}
function addVel(x,y,z,vx,vy,vz,color){
  const d=new THREE.Vector3(vx,vy,vz); if(d.length()<1e-9) return;
  const dir=d.clone().normalize();
  const a=new THREE.ArrowHelper(dir,new THREE.Vector3(x,y,z),0.6,0xffffff,0.22,0.14);
  scene.add(a);decals.push(a);
}
function addBonds(frame){
  const up=new THREE.Vector3(0,1,0);
  for(const bd of frame.bonds){
    const A=meshes.get(bd.a),B=meshes.get(bd.b); if(!A||!B) continue;
    const p1=A.position,p2=B.position;
    const dir=p2.clone().sub(p1); const len=dir.length(); if(len<1e-6) continue;
    if(bd.max!=null && len>bd.max) continue; if(bd.min!=null && len<bd.min) continue;
    const n=dir.clone().normalize();
    const ref=Math.abs(n.dot(up))>0.9? new THREE.Vector3(1,0,0): up;
    const v=new THREE.Vector3().crossVectors(n,ref.clone().addScaledVector(n,-n.dot(ref)).normalize());
    const order=Math.max(1,bd.order||1), pol=bd.polarity||0;
    for(let i=0;i<order;i++){
      const off=(i-(order-1)/2)*0.11;
      const m=new THREE.Mesh(new THREE.CylinderGeometry(0.05,0.05,len,8,1,true), new THREE.MeshStandardMaterial({color:0xcccccc,metalness:0.3,roughness:0.4,transparent:true,opacity:0.9}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5).addScaledVector(v,off);
      m.quaternion.setFromUnitVectors(up,n); scene.add(m);decals.push(m);
    }
    if(pol>0){
      const ca=new THREE.Color(A.material.color), cb=new THREE.Color(B.material.color);
      const m=new THREE.Mesh(new THREE.CylinderGeometry(0.058,0.058,len,8,1,true), new THREE.MeshStandardMaterial({color:ca.lerp(cb,0.5+0.5*pol),metalness:0.2,roughness:0.5,transparent:true,opacity:0.55}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5);
      m.quaternion.setFromUnitVectors(up,n); scene.add(m);decals.push(m);
    }
    if(bd.cloud){
      const geo=new THREE.SphereGeometry(1,16,12); geo.scale(0.16,0.16,len*0.7);
      const m=new THREE.Mesh(geo,new THREE.MeshStandardMaterial({color:0x66ccff,metalness:0.0,roughness:0.3,transparent:true,opacity:0.28}));
      m.position.copy(p1).add(p2).multiplyScalar(0.5);
      m.quaternion.setFromUnitVectors(new THREE.Vector3(0,0,1),n); scene.add(m);decals.push(m);
    }
  }
}
let alive=true, inflight=null;
function startLoop(){ renderer.setAnimationLoop(()=>{controls.update();composer.render();labelRenderer.render(scene,camera);}); }
window.__pwe=()=>({geo:renderer.info.memory.geometries,tex:renderer.info.memory.textures,prog:renderer.info.programs?renderer.info.programs.length:-1,meshes:meshes.size,decals:decals.length,children:scene.children.length});
// Stop polling and rendering, aborting any in-flight request. Called when the
// tab is hidden or torn down.
function stopAll(){
  alive=false;
  try{ if(inflight) inflight.abort(); }catch(e){}
  try{ renderer.setAnimationLoop(null); }catch(e){}
}
// On page hide navigate away / close, also release the WebGL context promptly:
// otherwise the browser drains the GPU context and a pending request, which is
// what makes closing the tab slow. No `beforeunload` handler (it can itself
// delay or prompt on close).
addEventListener('pagehide',()=>{
  stopAll();
  try{ renderer.dispose(); renderer.forceContextLoss(); }catch(e){}
});
document.addEventListener('visibilitychange',()=>{
  if(document.hidden){ stopAll(); }
  else { alive=true; startLoop(); poll(); }
});
async function poll(){
  if(!alive) return;
  const ac=new AbortController(); inflight=ac;
  try{const r=await fetch('/state',{cache:'no-store',signal:ac.signal});const f=await r.json();apply(f);conn.style.display='none';}
  catch(e){ if(e&&e.name==='AbortError') return; conn.style.display='block';conn.textContent='waiting for runtime…'; }
  inflight=null;
  if(alive) setTimeout(poll,60);
}
document.getElementById('rst').onclick=()=>{fetch('/reset').catch(()=>{});};
let paused=false;
document.getElementById('pse').onclick=()=>{paused=!paused;fetch('/pause?on='+(paused?1:0)).then(r=>r.text()).then(()=>{document.getElementById('pse').textContent=(paused?'▶ Resume':'⏸ Pause');}).catch(()=>{});};
document.getElementById('rst').addEventListener('click',()=>{paused=true;document.getElementById('pse').textContent='▶ Resume';});
document.getElementById('lbl').onclick=()=>{labelsOn=!labelsOn;document.getElementById('lbl').style.opacity=labelsOn?'1':'0.45';};
poll();
startLoop();
addEventListener('resize',()=>{camera.aspect=innerWidth/innerHeight;camera.updateProjectionMatrix();renderer.setSize(innerWidth,innerHeight);composer.setSize(innerWidth,innerHeight);labelRenderer.setSize(innerWidth,innerHeight);});
</script>
</body></html>"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Entity, Scene};
    use pwe_api::EntityId;

    fn scene_with_body() -> Scene {
        let mut scene = Scene::new(Vec3::new(0.0, -9.81, 0.0));
        let mut e = Entity::dynamic();
        e.transform = Some(crate::components::Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Default::default()
        });
        e.collider = Some(crate::components::Collider::sphere(0.5));
        scene.insert(EntityId(1), e);
        scene.sim_time = 4.0;
        scene
    }

    /// RFC-0041: the viewer must be self-contained (no CDN import map), or an
    /// unreachable network leaves the tab pending (the "hang on close").
    #[test]
    fn live_viewer_is_self_contained() {
        let html = live_viewer_html();
        assert!(!html.contains("unpkg.com"), "viewer must not load a CDN");
        assert!(!html.contains("https://") || !html.contains("three.module.js"));
        assert!(html.contains("/vendor/three/three.module.js"));
        assert!(html.contains("/vendor/three/addons/"));
        for f in [
            "/vendor/three/three.module.js",
            "/vendor/three/addons/controls/OrbitControls.js",
            "/vendor/three/addons/geometries/ConvexGeometry.js",
            "/vendor/three/addons/loaders/SVGLoader.js",
            "/vendor/three/addons/objects/MarchingCubes.js",
            "/vendor/three/addons/renderers/CSS2DRenderer.js",
            "/vendor/three/addons/math/ConvexHull.js",
            "/vendor/three/addons/postprocessing/EffectComposer.js",
            "/vendor/three/addons/postprocessing/Pass.js",
            "/vendor/three/addons/postprocessing/RenderPass.js",
            "/vendor/three/addons/postprocessing/UnrealBloomPass.js",
            "/vendor/three/addons/shaders/CopyShader.js",
            "/vendor/three/addons/shaders/LuminosityHighPassShader.js",
        ] {
            assert!(vendor_file(f).is_some(), "missing vendored asset {f}");
        }
        // Bloom pipeline is wired through the import map.
        assert!(html.contains("three/addons/postprocessing/UnrealBloomPass.js"));
    }

    #[test]
    fn snapshot_captures_entities_and_time() {
        let scene = scene_with_body();
        let frame = snapshot(&scene, None);
        assert!((frame.time - 4.0).abs() < 1e-9);
        assert_eq!(frame.entities.len(), 1);
        let vis = &frame.entities[0];
        assert!(matches!(vis.shape, Shape::Sphere { radius } if (radius - 0.5).abs() < 1e-9));
        assert!((vis.position.x - 1.0).abs() < 1e-9);
    }

    fn scene_with_field() -> Scene {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut f = crate::field::Field::new3(2, 2, 2, 1.0);
        f.set3(1, 1, 1, 0.75);
        scene.fields.insert("heat".to_string(), f);
        scene
    }

    #[test]
    fn snapshot_captures_3d_fields_and_json() {
        let frame = snapshot(&scene_with_field(), None);
        assert_eq!(frame.fields.len(), 1);
        let fl = &frame.fields[0];
        assert_eq!((fl.width, fl.height, fl.depth), (2, 2, 2));
        assert_eq!(fl.cells.len(), 8);
        assert!(fl.cells.contains(&0.75));
        let json = frame_to_json(&frame);
        assert!(json.contains("\"fields\":["), "{json}");
        assert!(json.contains("\"name\":\"heat\""));
        assert!(json.contains("\"depth\":2"));
        // Both viewers render fields.
        let html = template(&format!("[{json}]"));
        assert!(html.contains("renderFields"));
        assert!(html.contains("MarchingCubes"));
    }

    #[test]
    fn entity_render_overrides_apply() {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut e = Entity::dynamic();
        e.state = Some(crate::components::State::new(vec![0.0]));
        e.render = Some(crate::components::RenderStyle {
            orient: false,
            no_velocity: false,
            shape: Some(1),
            size: Some(2.0),
            size3: None,
            shape_name: None,
            parts: None,
            opacity: Some(0.4),
            glow: Some(1.5),
            label: Some(false),
        });
        scene.insert(EntityId(1), e);
        let frame = snapshot(&scene, None);
        let v = &frame.entities[0];
        assert!(matches!(v.shape, Shape::Sphere { radius } if (radius - 2.0).abs() < 1e-9));
        assert!((v.opacity - 0.4).abs() < 1e-9);
        assert!((v.glow - 1.5).abs() < 1e-9);
        assert!(!v.label);
        let json = frame_to_json(&frame);
        assert!(json.contains("\"opacity\":0.4"), "{json}");
        assert!(json.contains("\"glow\":1.5"));
        assert!(json.contains("\"label\":false"));
    }

    #[test]
    fn custom_shape_renders_as_group() {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut e = Entity::dynamic();
        e.state = Some(crate::components::State::new(vec![0.0]));
        e.render = Some(crate::components::RenderStyle {
            orient: false,
            no_velocity: false,
            shape_name: Some("gizmo".into()),
            parts: Some(vec![
                crate::components::ShapePart {
                    name: None,
                    kind: 3,
                    a: 0.05,
                    b: 0.3,
                    c: 0.05,
                    offset: (0.0, 0.0, 0.0),
                    path: None,
                    points: Vec::new(),
                    faces: Vec::new(),
                    scale: 1.0,
                    ..Default::default()
                },
                crate::components::ShapePart {
                    name: None,
                    kind: 1,
                    a: 0.1,
                    b: 0.1,
                    c: 0.1,
                    offset: (0.0, 0.2, 0.0),
                    path: None,
                    points: Vec::new(),
                    faces: Vec::new(),
                    scale: 1.0,
                    ..Default::default()
                },
            ]),
            ..Default::default()
        });
        scene.insert(EntityId(1), e);
        let frame = snapshot(&scene, None);
        assert!(
            matches!(&frame.entities[0].shape, Shape::Group(g) if g.len() == 2),
            "custom shape must be a group"
        );
        let json = frame_to_json(&frame);
        assert!(json.contains("\"kind\":\"group\""), "{json}");
        assert!(json.contains("\"parts\":["));
    }

    /// Cylinder / cone / plane (the new 3D primitives) reach the frame both as
    /// group parts — the path the language produces — and as top-level shapes.
    #[test]
    fn three_dimensional_primitives_serialize() {
        let part = |kind: u8, a: f64, b: f64| crate::components::ShapePart {
            name: None,
            kind,
            a,
            b,
            c: 0.0,
            offset: (0.0, 0.0, 0.0),
            path: None,
            points: Vec::new(),
            faces: Vec::new(),
            scale: 1.0,
            ..Default::default()
        };
        let mut scene = Scene::new(Vec3::ZERO);
        let mut e = Entity::dynamic();
        e.state = Some(crate::components::State::new(vec![0.0]));
        e.render = Some(crate::components::RenderStyle {
            shape_name: Some("tower".into()),
            parts: Some(vec![
                part(9, 0.5, 2.0),
                part(10, 0.5, 1.0),
                part(11, 4.0, 3.0),
            ]),
            ..Default::default()
        });
        scene.insert(EntityId(1), e);
        let mut frame = snapshot(&scene, None);
        assert!(
            matches!(&frame.entities[0].shape, Shape::Group(g) if g.len() == 3),
            "cylinder+cone+plane shape must be a group"
        );
        let json = frame_to_json(&frame);
        assert!(json.contains("\"k\":9"), "cylinder part: {json}");
        assert!(json.contains("\"k\":10"), "cone part: {json}");
        assert!(json.contains("\"k\":11"), "plane part: {json}");
        // Top-level arms (a directly built frame can carry them).
        frame.entities[0].shape = Shape::Cylinder {
            radius: 0.5,
            height: 2.0,
        };
        let json = frame_to_json(&frame);
        assert!(
            json.contains("\"kind\":\"cylinder\",\"radius\":0.5,\"height\":2"),
            "{json}"
        );
        frame.entities[0].shape = Shape::Cone {
            radius: 0.5,
            height: 1.0,
        };
        let json = frame_to_json(&frame);
        assert!(
            json.contains("\"kind\":\"cone\",\"radius\":0.5,\"height\":1"),
            "{json}"
        );
        frame.entities[0].shape = Shape::Plane {
            width: 4.0,
            height: 3.0,
        };
        let json = frame_to_json(&frame);
        assert!(
            json.contains("\"kind\":\"plane\",\"w\":4,\"h\":3"),
            "{json}"
        );
    }

    /// The optional `vis` material block: omitted entirely when unset (older
    /// viewers and exact-JSON expectations keep working), and emitted with only
    /// the fields that are actually set.
    #[test]
    fn vis_block_is_optional_and_serialized() {
        let mut frame = snapshot(&scene_with_body(), None);
        let plain = frame_to_json(&frame);
        assert!(!plain.contains("\"vis\":"), "absent when unset: {plain}");
        frame.entities[0].style = Some(VisualStyle {
            metalness: Some(0.9),
            roughness: Some(0.2),
            emissive: Some(0xFF_00_00),
            emissive_intensity: Some(2.0),
            wireframe: Some(true),
            flat: Some(true),
            double_sided: Some(true),
            cast_shadow: Some(false),
        });
        let json = frame_to_json(&frame);
        assert!(json.contains("\"vis\":{"), "{json}");
        assert!(json.contains("\"metalness\":0.9"), "{json}");
        assert!(json.contains("\"roughness\":0.2"), "{json}");
        assert!(json.contains("\"emissive\":16711680"), "{json}");
        assert!(json.contains("\"emissiveIntensity\":2"), "{json}");
        assert!(json.contains("\"wireframe\":true"), "{json}");
        assert!(json.contains("\"flat\":true"), "{json}");
        assert!(json.contains("\"doubleSided\":true"), "{json}");
        assert!(json.contains("\"castShadow\":false"), "{json}");
        // Partially set: only the set fields appear.
        frame.entities[0].style = Some(VisualStyle {
            metalness: Some(0.5),
            ..Default::default()
        });
        let json = frame_to_json(&frame);
        assert!(json.contains("\"metalness\":0.5"), "{json}");
        assert!(!json.contains("roughness"), "{json}");
        assert!(!json.contains("castShadow"), "{json}");
    }

    #[test]
    fn frame_json_is_valid_shape() {
        let frame = snapshot(&scene_with_body(), None);
        let json = frame_to_json(&frame);
        // Contains the sphere radius and the position.
        assert!(json.contains("\"radius\":0.5"));
        assert!(json.contains("\"pos\":[1,2,3]"));
        assert!(json.contains("\"time\":4"));
        assert!(json.contains("\"camera\":null"));
    }

    #[test]
    fn viewer_template_embeds_frames() {
        let json = frame_to_json(&snapshot(&scene_with_body(), None));
        let html = template(&format!("[{json}]"));
        assert!(html.contains("PWE 3D viewport"));
        // The frames placeholder is substituted.
        assert!(html.contains(&format!("[{json}]")));
    }

    /// `write_viewer` output must be a genuine single file: the vendored
    /// three.js modules ride inside the document and are loaded through blob
    /// URLs, so the page opens straight from `file://` with no server route,
    /// no import map and no CDN.
    #[test]
    fn static_viewer_is_one_self_contained_file() {
        let json = frame_to_json(&snapshot(&scene_with_body(), None));
        let html = template(&format!("[{json}]"));
        // Embedded vendored sources + blob bootstrap.
        assert!(html.contains("id=\"pwe-vendor\""), "vendor payload missing");
        assert!(
            html.contains("URL.createObjectURL"),
            "blob bootstrap missing"
        );
        assert!(html.contains("\"three\":\""), "three.module.js not inlined");
        assert!(
            html.contains("SPDX-License-Identifier: MIT"),
            "vendor truncated"
        );
        // The only relative import inside the addons is rewritten to a bare
        // specifier the bootstrap can resolve.
        assert!(!html.contains("'../math/ConvexHull.js'"));
        assert!(html.contains("three/addons/math/ConvexHull.js"));
        // No server-relative route, no import map (blocked on file://), no CDN.
        assert!(!html.contains("/vendor/three/"));
        assert!(!html.contains("<script type=\"importmap\">"));
        assert!(!html.contains("unpkg.com"));
        // Both viewers escape user-authored strings before innerHTML sinks.
        assert!(html.contains("const esc ="));
        // Realistic presentation: ACES tone mapping, bloom composer, a
        // shadow-casting key light, data-driven primitives and fading trails.
        assert!(html.contains("ACESFilmicToneMapping"));
        assert!(html.contains("UnrealBloomPass"));
        assert!(html.contains("renderer.shadowMap.enabled"));
        assert!(html.contains("o.castShadow"));
        assert!(html.contains("const PRIMS ="));
        assert!(html.contains("function updateTrails"));
        // The postprocessing modules ride inside the same document.
        assert!(
            html.contains("\"three/addons/postprocessing/UnrealBloomPass.js\":\""),
            "postprocessing payload missing"
        );
    }

    /// Names, field names and info strings are user-authored text; they must
    /// never be able to close the data `<script>` element or inject markup.
    #[test]
    fn frame_json_cannot_break_out_of_script_or_markup() {
        let mut scene = Scene::new(Vec3::ZERO);
        scene.insert(EntityId(1), Entity::dynamic());
        let mut names = std::collections::BTreeMap::new();
        names.insert(1u128, "</script><script>alert(1)</script>\n\t".to_string());
        let frame = snapshot_with(&names, &[], &scene, None);
        let json = frame_to_json(&frame);
        assert!(!json.contains("</script>"), "raw tag leaked: {json}");
        assert!(!json.contains("<script"), "raw tag leaked: {json}");
        assert!(
            json.contains("\\u003c/script\\u003e"),
            "missing escape: {json}"
        );
        assert!(
            json.contains("\\n") && json.contains("\\t"),
            "missing escapes: {json}"
        );
        let html = template(&format!("[{json}]"));
        assert!(!html.contains("<script>alert(1)"));
        // Live info lines get the same treatment.
        let live = LiveState {
            frame,
            step: 1,
            info: vec!["</script>x".into()],
        };
        let lj = live_state_json(&live);
        assert!(!lj.contains("</script>"), "raw tag leaked: {lj}");
    }

    /// The 7-slot body state must serialize as a compact JSON array.
    #[test]
    fn state_slots_serialize_as_a_json_array() {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut e = Entity::dynamic();
        e.state = Some(crate::components::State::new(vec![
            4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]));
        scene.insert(EntityId(1), e);
        let json = frame_to_json(&snapshot(&scene, None));
        // The body state is a fixed 16-slot register file; it serializes as a
        // compact JSON array of slot values.
        assert!(
            json.contains("\"state\":[4,2,0,0,0,0,1,0,0,0,0,0,0,0,0,0]"),
            "{json}"
        );
    }

    #[test]
    fn live_state_json_includes_step_frame_and_info() {
        let frame = snapshot(&scene_with_body(), None);
        let live = LiveState {
            frame,
            step: 42,
            info: vec!["gravity ran".to_string(), "vehicle y=0.8".to_string()],
        };
        let json = live_state_json(&live);
        assert!(json.contains("\"step\":42"));
        assert!(json.contains("\"time\":4"));
        assert!(json.contains("gravity ran"));
        assert!(json.contains("\"info\":["));
    }

    #[test]
    fn live_viewer_page_polls_state() {
        let page = live_viewer_html();
        assert!(page.contains("PWE live 3D viewport"));
        assert!(page.contains("fetch('/state'"));
        // RFC-0041: closing the tab must release the WebGL context promptly and
        // abort the in-flight poll (otherwise the close can stall).
        assert!(page.contains("forceContextLoss"));
        assert!(page.contains("AbortController"));
        assert!(page.contains("pagehide"));
        // User-authored names/info are escaped before innerHTML sinks.
        assert!(page.contains("const esc=s=>"));
        // Selection walks up the parent chain to the entity id (opaque ids).
        assert!(page.contains("userData.id"));
        // Leak guard: every per-frame removal must dispose GPU/DOM resources,
        // or memory (and close time) grows with runtime.
        assert!(page.contains("function disposeObj"));
        assert!(page.contains("function disposeMat"));
        assert!(page.contains("scene.remove(m); disposeObj(m)"));
        assert!(page.contains("OrbitControls"));
        // Realistic presentation: ACES tone mapping, bloom composer, a
        // shadow-casting key light, data-driven primitives and rolling trails.
        assert!(page.contains("ACESFilmicToneMapping"));
        assert!(page.contains("UnrealBloomPass"));
        assert!(page.contains("composer.render()"));
        assert!(page.contains("renderer.shadowMap.enabled"));
        assert!(page.contains("o.castShadow"));
        assert!(page.contains("const PRIMS ="));
        assert!(page.contains("function updateTrails"));
    }

    #[test]
    fn entity_color_is_preserved_and_framed() {
        let mut scene = scene_with_body();
        if let Some(e) = scene.entities.get_mut(&EntityId(1)) {
            e.color = Some(0x00_FF_00);
        }
        let frame = snapshot(&scene, None);
        assert_eq!(frame.entities[0].color_value(), 0x00_FF_00);
        let json = frame_to_json(&frame);
        assert!(json.contains("\"color\":65280")); // 0x00FF00 = 65280
    }

    #[test]
    fn state_only_body_is_an_entity_not_a_channel() {
        // An nbody particle has no Transform; its position is state[0..2].
        let mut scene = Scene::new(Vec3::ZERO);
        let mut e = Entity::dynamic();
        e.state = Some(crate::components::State::new(vec![
            4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]));
        scene.insert(EntityId(1), e);
        let frame = snapshot(&scene, None);
        // It must be a body (entity) at state position, not a channel.
        assert_eq!(frame.entities.len(), 1);
        assert!(frame.channels.is_empty());
        assert!((frame.entities[0].position.x - 4.0).abs() < 1e-9);
        assert!((frame.entities[0].position.y - 2.0).abs() < 1e-9);
    }

    #[test]
    fn channel_ids_are_reported_as_channels() {
        let mut scene = Scene::new(Vec3::ZERO);
        let mut ch = Entity::dynamic();
        ch.state = Some(crate::components::State::new(vec![42.0]));
        scene.insert(EntityId(1), ch);
        let frame = snapshot_with(&Default::default(), &[1], &scene, None);
        assert!(frame.entities.is_empty());
        assert_eq!(frame.channels.len(), 1);
        assert_eq!(frame.channels[0].value, 42.0);
    }
}

// ---------------------------------------------------------------------------
// Playground: an editor + live viewer served locally. The browser submits
// source to `POST /api/source`; a single driver thread (owning the runtime, so
// no cross-thread `Send` requirement on backends) compiles it, steps it, and
// publishes frames to the same `/state` the `present` viewer polls.
// ---------------------------------------------------------------------------

enum PgCmd {
    Load(String, std::sync::mpsc::Sender<String>),
    Reset,
    /// `Some(true)` pause, `Some(false)` resume, `None` toggle.
    Pause(Option<bool>),
}

/// Serves the PWE playground on `127.0.0.1:port` (editor at `/`, viewer at
/// `/view`). Returns once the listener is bound; work happens on background
/// threads (Ctrl-C stops the process).
pub fn serve_playground(port: u16) -> std::io::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<PgCmd>();
    let state = Arc::new(RwLock::new(LiveState::default()));
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    // Per-process CSRF token: state-changing requests must present it. A
    // cross-site page cannot read it (no CORS) or set the header without a
    // preflight (rejected), so it cannot forge a request — and no-token requests
    // (e.g. a bare `curl`) are refused too.
    let token = format!(
        "{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ std::process::id() as u64
    );
    let page = playground_html(&token);
    let viewer = viewer_with_token(&live_viewer_html(), &token);
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || playground_driver(rx, state));
    }
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let state = Arc::clone(&state);
            let page = page.clone();
            let viewer = viewer.clone();
            let token = token.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = handle_playground(&mut stream, &tx, &state, &page, &viewer, &token);
            });
        }
    });
    Ok(())
}

fn publish(state: &Arc<RwLock<LiveState>>, rt: &crate::lang::LangRuntime, step: u64, note: &str) {
    let frame = rt.present_frame(None);
    let info = vec![if note.is_empty() {
        format!("step {step}")
    } else {
        format!("{note} · step {step}")
    }];
    let mut g = state.write().unwrap_or_else(|e| e.into_inner());
    g.step = step;
    g.frame = frame;
    g.info = info;
}

fn playground_driver(rx: std::sync::mpsc::Receiver<PgCmd>, state: Arc<RwLock<LiveState>>) {
    let mut rt: Option<crate::lang::LangRuntime> = None;
    let mut last_src: Option<String> = None;
    let mut paused = true;
    let mut step = 0u64;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(16)) {
            Ok(PgCmd::Load(src, reply)) => match crate::lang::LangRuntime::compile(&src) {
                Ok(r) => {
                    rt = Some(r);
                    last_src = Some(src);
                    step = 0;
                    paused = false;
                    let _ = reply.send("ok".to_string());
                    if let Some(r) = &rt {
                        publish(&state, r, 0, "compiled");
                    }
                }
                Err(e) => {
                    let _ = reply.send(crate::lang::diagnose(&src, &e));
                }
            },
            Ok(PgCmd::Reset) => {
                if let Some(src) = last_src.clone() {
                    if let Ok(r) = crate::lang::LangRuntime::compile(&src) {
                        rt = Some(r);
                        step = 0;
                        if let Some(r) = &rt {
                            publish(&state, r, 0, "reset");
                        }
                    }
                }
            }
            Ok(PgCmd::Pause(p)) => paused = p.unwrap_or(!paused),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if !paused {
                    if let Some(r) = &mut rt {
                        match r.step_interpreter() {
                            Ok(_) => {
                                step += 1;
                                publish(&state, r, step, "");
                            }
                            Err(e) => {
                                paused = true;
                                let mut info = vec![format!("step {step} failed: {e}")];
                                for d in crate::lang::take_diagnostics() {
                                    info.push(format!("[{}] {}", d.detail, d.message));
                                }
                                let frame = r.present_frame(None);
                                let mut g = state.write().unwrap_or_else(|e| e.into_inner());
                                g.step = step;
                                g.frame = frame;
                                g.info = info;
                            }
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn read_http(stream: &mut TcpStream) -> std::io::Result<(String, String, String, Vec<u8>)> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..pos]).to_string();
            let method = head.split_whitespace().next().unwrap_or("GET").to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            let content_length = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            let body_start = pos + 4;
            while data.len() < body_start + content_length {
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            let end = (body_start + content_length).min(data.len());
            let body = data[body_start..end].to_vec();
            return Ok((method, path, head, body));
        }
    }
    Ok((
        "GET".to_string(),
        "/".to_string(),
        String::new(),
        Vec::new(),
    ))
}

fn header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

/// Whether an `Origin` header names the local server (exact host match — a
/// prefix match would accept `localhost.evil.com`).
fn origin_is_local(origin: &str) -> bool {
    let rest = origin.split("://").nth(1).unwrap_or(origin);
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped).to_string()
    } else {
        authority.split(':').next().unwrap_or(authority).to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
}

fn respond(stream: &mut TcpStream, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    respond_status(stream, "200 OK", ctype, body)
}

fn respond_status(
    stream: &mut TcpStream,
    code: &str,
    ctype: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn handle_playground(
    stream: &mut TcpStream,
    tx: &std::sync::mpsc::Sender<PgCmd>,
    state: &Arc<RwLock<LiveState>>,
    page: &str,
    viewer: &str,
    token: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_millis(2000)))?;
    let (method, raw_path, head, body) = read_http(stream)?;
    let (mut path, query) = match raw_path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_path, String::new()),
    };
    // Alias the viewer's control paths onto the guarded `api` handlers.
    if path == "/reset" {
        path = "/api/reset".to_string();
    } else if path == "/pause" {
        path = "/api/pause".to_string();
    }

    // CSRF guard: state-changing requests must carry the per-process token and
    // must not be cross-site. A cross-site page cannot read the token (no CORS)
    // nor set the header without a preflight, and a non-local `Origin` (exact
    // host) or `Sec-Fetch-Site: cross-site` is refused. No-token requests (e.g.
    // a bare `curl`) are refused too.
    let state_changing = path.starts_with("/api/");
    if state_changing {
        if let Some(o) = header(&head, "origin") {
            if !origin_is_local(&o) {
                return respond_status(
                    stream,
                    "403 Forbidden",
                    "text/plain",
                    b"cross-origin denied",
                );
            }
        }
        if header(&head, "sec-fetch-site").as_deref() == Some("cross-site") {
            return respond_status(stream, "403 Forbidden", "text/plain", b"cross-site denied");
        }
        if header(&head, "x-pwe-token").unwrap_or_default() != token {
            return respond_status(
                stream,
                "403 Forbidden",
                "text/plain",
                b"missing/invalid token",
            );
        }
    }

    if method == "POST" && path == "/api/source" {
        let src = String::from_utf8_lossy(&body).to_string();
        let (rtx, rrx) = std::sync::mpsc::channel::<String>();
        if tx.send(PgCmd::Load(src, rtx)).is_err() {
            return respond(stream, "text/plain; charset=utf-8", b"driver stopped");
        }
        let msg = rrx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| "compile timed out".to_string());
        return respond(stream, "text/plain; charset=utf-8", msg.as_bytes());
    }
    if path == "/api/reset" {
        let _ = tx.send(PgCmd::Reset);
        return respond(stream, "text/plain", b"ok");
    }
    if path == "/api/pause" {
        let set = if query.contains("on=0") {
            Some(false)
        } else if query.contains("on=1") {
            Some(true)
        } else {
            None
        };
        let _ = tx.send(PgCmd::Pause(set));
        return respond(stream, "text/plain", b"ok");
    }
    if path == "/view" {
        return respond(stream, "text/html; charset=utf-8", viewer.as_bytes());
    }
    if path == "/" {
        return respond(stream, "text/html; charset=utf-8", page.as_bytes());
    }
    if let Some(asset) = vendor_file(&path) {
        return respond(stream, "text/javascript; charset=utf-8", asset.as_bytes());
    }
    if path == "/state" {
        let live = state.read().unwrap_or_else(|e| e.into_inner());
        return respond(
            stream,
            "application/json",
            live_state_json(&live).as_bytes(),
        );
    }
    respond(stream, "text/plain", b"not found")
}

/// Injects the CSRF token into the `present` viewer page and rewrites its
/// control endpoints (`/reset`, `/pause`) onto the guarded `/api/*` routes.
fn viewer_with_token(viewer: &str, token: &str) -> String {
    let patch = format!(
        "<script>(function(){{const T={token:?};const f=window.fetch;window.fetch=function(u,o){{if(typeof u==='string'){{u=u.replace(/^\\/reset/,'/api/reset').replace(/^\\/pause/,'/api/pause');}}o=o||{{}};o.headers=Object.assign({{}},o.headers,{{'X-PWE-Token':T}});return f(u,o);}};}})();</script>"
    );
    match viewer.replacen("<head>", &format!("<head>{patch}"), 1) {
        s if s.contains(&patch) => s,
        _ => format!("{patch}{viewer}"),
    }
}

/// The playground page: a plain-text editor, Run, error pane, and the viewer in
/// an iframe (reloaded on a successful compile). All fetches carry the CSRF
/// token (`X-PWE-Token`).
fn playground_html(token: &str) -> String {
    r#"<!doctype html><html><head><meta charset="utf-8">
<title>PWE playground</title>
<style>
  html,body{margin:0;height:100%;font-family:ui-monospace,Menlo,monospace;background:#0b0e14;color:#cdd6f4}
  #wrap{display:flex;height:100%}
  #left{width:44%;min-width:320px;display:flex;flex-direction:column;border-right:1px solid #313244}
  #bar{padding:8px;display:flex;gap:8px;align-items:center;background:#11131a}
  button{background:#3a4a6b;color:#fff;border:0;padding:6px 12px;border-radius:5px;cursor:pointer}
  button:hover{background:#4a5f88}
  textarea{flex:1;width:100%;box-sizing:border-box;background:#0b0e14;color:#cdd6f4;border:0;padding:12px;
           font:13px/1.5 ui-monospace,Menlo,monospace;resize:none;outline:none}
  #err{padding:8px 12px;color:#f38ba8;white-space:pre-wrap;font-size:12px;min-height:0;max-height:30%;overflow:auto}
  #view{flex:1;border:0;background:#0b0e14}
  .hint{color:#6c7086;font-size:12px}
</style></head><body>
<div id="wrap">
  <div id="left">
    <div id="bar"><button id="run">Run</button><button id="reset">Reset</button>
      <span class="hint">Ctrl/Cmd-Enter to run</span></div>
    <textarea id="src" spellcheck="false"></textarea>
    <div id="err"></div>
  </div>
  <iframe id="view" src="/view"></iframe>
</div>
<script>
const TOKEN = "__PWE_TOKEN__";
(function(){const f=window.fetch;window.fetch=function(u,o){o=o||{};o.headers=Object.assign({},o.headers,{"X-PWE-Token":TOKEN});return f(u,o);};})();
const SAMPLE = `world {
  gravity = (0, -9.81, 0)
  entity ball  { position = (0, 6, 0); velocity = (1.2, 0, 0); shape = sphere; color = 0x89b4fa }
  entity ball2 { position = (1.5, 9, 0); velocity = (-0.8, 0, 0); shape = sphere; color = 0xf38ba8 }
}
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.85 }
}`;
const src = document.getElementById('src');
src.value = SAMPLE;
const err = document.getElementById('err');
const view = document.getElementById('view');
async function run(){
  err.textContent = 'compiling…';
  try{
    const r = await fetch('/api/source', { method:'POST', body: src.value });
    const t = await r.text();
    if (t === 'ok'){ err.textContent = ''; view.src = '/view?t=' + Date.now(); }
    else { err.textContent = t; }
  }catch(e){ err.textContent = String(e); }
}
document.getElementById('run').onclick = run;
document.getElementById('reset').onclick = () => fetch('/api/reset').then(()=>{ view.src='/view?t='+Date.now(); });
src.addEventListener('keydown', e => { if((e.metaKey||e.ctrlKey) && e.key==='Enter'){ e.preventDefault(); run(); }});
</script></body></html>"#
        .replace("__PWE_TOKEN__", token)
}

#[cfg(test)]
mod playground_tests {
    #[test]
    fn playground_page_has_editor_viewer_and_token() {
        let html = super::playground_html("deadbeefcafef00d");
        for needle in [
            "id=\"src\"",
            "id=\"run\"",
            "/api/source",
            "/view",
            "SAMPLE",
            "X-PWE-Token",
            "deadbeefcafef00d",
        ] {
            assert!(html.contains(needle), "playground page missing {needle}");
        }
    }

    #[test]
    fn viewer_gets_token_and_rewritten_controls() {
        let v = super::viewer_with_token("<head></head><body>viewer</body>", "tok123");
        assert!(v.contains("X-PWE-Token"));
        assert!(v.contains("tok123"));
        assert!(v.contains("/api/reset") && v.contains("/api/pause"));
    }

    #[test]
    fn origin_host_match_is_exact() {
        assert!(super::origin_is_local("http://localhost:8080"));
        assert!(super::origin_is_local("http://127.0.0.1"));
        assert!(super::origin_is_local("https://localhost"));
        assert!(!super::origin_is_local("http://localhost.evil.com"));
        assert!(!super::origin_is_local("http://127.0.0.1.evil.com"));
        assert!(!super::origin_is_local("https://evil.example"));
    }
}
