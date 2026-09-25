//! Presentation layer: turn a running simulation's `Scene` into 3D web frames.
//!
//! [`snapshot`] captures the world state (entity transforms + collider shapes +
//! state slots + channels + camera) into a [`PresentationFrame`]; [`to_json`]
//! serializes frames, and [`write_viewer`] emits a self-contained HTML file with
//! a Three.js 3D viewport that plays them back in a browser.
//!
//! The viewer is generated output (a single `.html`), not a build dependency —
//! Three.js is loaded from a CDN at runtime. This makes a running simulation
//! visibly observable, including a micro/macro world at any scale.

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
    Group(Vec<(Shape, Vec3)>),
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
    /// Bonded entity-id pairs (for molecule rendering): drawn as lines between
    /// the two atoms' positions.
    pub bonds: Vec<(u128, u128)>,
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

/// Captures the current `Scene` into a presentation frame.
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
                // Self-rotation (自转): state[7] is a spin angle about Z.
                let ang = state.get(7).copied().unwrap_or(0.0);
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: (ang * 0.5).sin(),
                    w: (ang * 0.5).cos(),
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
                    let group: Vec<(Shape, Vec3)> = parts
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
                                _ => Shape::Point,
                            };
                            (
                                shape,
                                Vec3::new(
                                    p.offset.0 * whole,
                                    p.offset.1 * whole,
                                    p.offset.2 * whole,
                                ),
                            )
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
pub fn with_bonds(frame: &PresentationFrame, bonds: &[(u128, u128)]) -> PresentationFrame {
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

/// Serializes one frame to a JSON object string (no external serde).
pub fn frame_to_json(frame: &PresentationFrame) -> String {
    let mut out = String::new();
    let _ = write!(out, "{{\"time\":{},\"entities\":[", fmt_f64(frame.time));
    for (i, e) in frame.entities.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{},\"name\":\"{}\",\"pos\":{},\"rot\":{},\"color\":{},\"opacity\":{},\"glow\":{},\"label\":{},\"kind\":",
            e.id,
            e.name.replace('\\', "\\\\").replace('"', "\\\""),
            vec3_json(e.position),
            quat_json(e.rotation),
            e.color_value(),
            fmt_f64(e.opacity),
            fmt_f64(e.glow),
            e.label
        ));
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
                    path.replace('\\', "\\\\").replace('"', "\\\""),
                    fmt_f64(*depth),
                    fmt_f64(*scale)
                ));
            }
            Shape::Group(parts) => {
                out.push_str("\"group\",\"parts\":[");
                for (j, (shape, off)) in parts.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!("{{\"off\":{},", vec3_json(*off)));
                    match shape {
                        Shape::Sphere { radius } => {
                            out.push_str(&format!("\"k\":1,\"a\":{}}}", fmt_f64(*radius)));
                        }
                        Shape::Box { dims } => {
                            out.push_str(&format!(
                                "\"k\":2,\"a\":{},\"b\":{},\"c\":{}}}",
                                fmt_f64(dims.x),
                                fmt_f64(dims.y),
                                fmt_f64(dims.z)
                            ));
                        }
                        Shape::Capsule {
                            radius_bottom,
                            length,
                            radius_top,
                        } => {
                            out.push_str(&format!(
                                "\"k\":3,\"a\":{},\"b\":{},\"c\":{}}}",
                                fmt_f64(*radius_bottom),
                                fmt_f64(*length),
                                fmt_f64(*radius_top)
                            ));
                        }
                        Shape::Svg { path, depth, scale } => {
                            out.push_str(&format!(
                                "\"k\":4,\"d\":\"{}\",\"depth\":{},\"scale\":{}}}",
                                path.replace('\\', "\\\\").replace('"', "\\\""),
                                fmt_f64(*depth),
                                fmt_f64(*scale)
                            ));
                        }
                        Shape::Hull { points } => {
                            out.push_str("\"k\":5,\"pts\":[");
                            for (k, p) in points.iter().enumerate() {
                                if k > 0 {
                                    out.push(',');
                                }
                                out.push_str(&vec3_json(*p));
                            }
                            out.push_str("]}");
                        }
                        Shape::Poly { points, faces } => {
                            out.push_str("\"k\":6,\"pts\":[");
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
                            out.push_str("]}");
                        }
                        _ => out.push_str("\"k\":0}"),
                    }
                }
                out.push(']');
                out.push_str(",\"state\":[");
                for (j, s) in e.state.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&fmt_f64(*s));
                }
                out.push_str("]}");
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
                out.push_str(",\"state\":[");
                for (j, s) in e.state.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&fmt_f64(*s));
                }
                out.push_str("]}");
            }
            Shape::Point => {
                out.push_str("\"point\",\"size\":");
                out.push_str(&fmt_f64(e.size));
                out.push_str(",\"state\":[");
                for (j, s) in e.state.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    out.push_str(&fmt_f64(*s));
                }
                out.push_str("]}");
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
            "{{\"name\":\"{}\",\"width\":{},\"height\":{},\"depth\":{},\"dx\":{},\"stride\":{},\"cells\":[",
            f.name.replace('\\', "\\\\").replace('"', "\\\""),
            f.width,
            f.height,
            f.depth,
            fmt_f64(f.dx),
            f.stride
        );
        for (j, c) in f.cells.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&fmt_f64(*c));
        }
        out.push_str("]}");
    }
    out.push_str("],\"bonds\":[");
    for (i, (a, b)) in frame.bonds.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("[{a},{b}]"));
    }
    out.push_str("]}");
    out
}

/// Writes a self-contained 3D web viewport over `frames` to `path`. Open the
/// generated `.html` in a browser to watch the simulation play back.
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

fn template(frames_json: &str) -> String {
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
<script type="importmap">{{
  "imports": {{
    "three": "/vendor/three/three.module.js",
    "three/addons/": "/vendor/three/addons/"
  }}
}}</script>
<script type="module">
import * as THREE from 'three';
import {{ OrbitControls }} from 'three/addons/controls/OrbitControls.js';
import {{ ConvexGeometry }} from 'three/addons/geometries/ConvexGeometry.js';
import {{ SVGLoader }} from 'three/addons/loaders/SVGLoader.js';
import {{ MarchingCubes }} from 'three/addons/objects/MarchingCubes.js';
import {{ CSS2DRenderer, CSS2DObject }} from 'three/addons/renderers/CSS2DRenderer.js';
const FRAMES = {frames_json};
let idx = 0, playing = false;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0b0e14);
const gridHelp = new THREE.GridHelper(20, 20, 0x2a3240, 0x1a2030); scene.add(gridHelp);
const axesHelp = new THREE.AxesHelper(2); scene.add(axesHelp);
scene.add(new THREE.AmbientLight(0xffffff, 0.5));
scene.add(new THREE.HemisphereLight(0x9cc4ff, 0x0b0e14, 0.9));
const dl = new THREE.DirectionalLight(0xffffff, 0.8); dl.position.set(8, 14, 10); scene.add(dl);
const sunLight = new THREE.PointLight(0xFFD24A, 2, 100); scene.add(sunLight);
const camera = new THREE.PerspectiveCamera(60, innerWidth/innerHeight, 0.01, 1000);
camera.position.set(8, 8, 8);
const renderer = new THREE.WebGLRenderer({{antialias:true}});
renderer.setSize(innerWidth, innerHeight);
document.getElementById('view').appendChild(renderer.domElement);
const labelRenderer = new CSS2DRenderer(); labelRenderer.setSize(innerWidth, innerHeight); labelRenderer.domElement.style.position='absolute'; labelRenderer.domElement.style.top='0'; labelRenderer.domElement.style.pointerEvents='none'; document.getElementById('view').appendChild(labelRenderer.domElement);
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
const highlight=new THREE.Mesh(new THREE.SphereGeometry(0.4,16,16),new THREE.MeshBasicMaterial({{color:0xffffff,wireframe:true,transparent:true,opacity:0.5}}));highlight.visible=false;scene.add(highlight);
let selected=null;
renderer.domElement.addEventListener('pointerdown',(ev)=>{{
  const rect=renderer.domElement.getBoundingClientRect();
  pointer.x=((ev.clientX-rect.left)/rect.width)*2-1; pointer.y=-((ev.clientY-rect.top)/rect.height)*2+1;
  raycaster.setFromCamera(pointer,camera);
  const hits=raycaster.intersectObjects([...meshes.values()]);
  selected=hits.length?hits[0].object:null;
}});
const panel = document.getElementById('panel');
const meshes = new Map();
function svgGeo(d, depth, scale){{ const data=new SVGLoader().parse(d); let sh=[]; for(const p of data.paths) sh=sh.concat(SVGLoader.createShapes(p));
  const geo=new THREE.ExtrudeGeometry(sh,{{depth:Math.max(depth,0.001),bevelEnabled:false,curveSegments:16}}); geo.scale(scale,-scale,scale); geo.center(); return geo; }}
function polyGeo(pts, faces){{ const pos=[]; const F=faces&&faces.length?faces:null;
  if(F){{ for(const f of F){{ for(let i=1;i+1<f.length;i++){{ for(const q of [f[0],f[i],f[i+1]]){{ const p=pts[q]; pos.push(p[0],p[1],p[2]); }} }} }} }}
  const g=new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3)); g.computeVertexNormals(); return g; }}
function capsuleGeo(r0,len,r1){{ const cs=8, pts=[];
  for(let i=0;i<=cs;i++){{const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r0*Math.sin(a), -len/2 - r0*Math.cos(a)));}}
  for(let i=0;i<=cs;i++){{const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r1*Math.cos(a), len/2 + r1*Math.sin(a)));}}
  return new THREE.LatheGeometry(pts,28); }}
function makeMesh(e) {{
  const opacity = e.opacity==null?1:e.opacity, glow = e.glow==null?0.8:e.glow;
  const mat = new THREE.MeshStandardMaterial({{color: e.color,emissive:new THREE.Color(e.color),emissiveIntensity:glow,metalness:0.0,roughness:0.5,transparent:opacity<1,opacity:opacity}});
  if (e.kind==='box') return new THREE.Mesh(new THREE.BoxGeometry(e.dims[0],e.dims[1],e.dims[2]), mat);
  if (e.kind==='sphere') return new THREE.Mesh(new THREE.SphereGeometry(e.radius,20,16), mat);
  if (e.kind==='hull' && e.points) {{
    const verts = e.points.map(p=>new THREE.Vector3(p[0],p[1],p[2]));
    let g; try {{ g = new ConvexGeometry(verts); }} catch(err) {{ g = new THREE.SphereGeometry(0.1,8,6); }}
    return new THREE.Mesh(g, mat);
  }}
  if (e.kind==='capsule') return new THREE.Mesh(capsuleGeo(e.r0||0.06, e.len||0.3, e.r1||0.06), mat);
  if (e.kind==='group' && e.parts) {{ const g=new THREE.Group(); for (const p of e.parts) {{ let ch;
    if (p.k===2) {{ ch=new THREE.Mesh(new THREE.BoxGeometry(p.a,p.b,p.c), mat); }}
    else if (p.k===3) {{ ch=new THREE.Mesh(capsuleGeo(p.a,p.b,p.c), mat); }}
    else if (p.k===4) {{ ch=new THREE.Mesh(svgGeo(p.d, p.depth, p.scale), mat); }}
    else if (p.k===5) {{ ch=new THREE.Mesh(new ConvexGeometry((p.pts||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2]))), mat); }}
    else if (p.k===6) {{ ch=new THREE.Mesh(polyGeo(p.pts||[], p.faces||[]), mat); }}
    else {{ ch=new THREE.Mesh(new THREE.SphereGeometry(p.a,20,16), mat); }}
    ch.position.set(p.off[0],p.off[1],p.off[2]); g.add(ch); }} return g; }}
  return new THREE.Mesh(new THREE.SphereGeometry((e.size||0.25)/2,20,16), mat);
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
function clearFields() {{ for (const o of fieldObjects.values()) {{ if (o.mc) {{ scene.remove(o.mc); scene.remove(o.trough.mc); }} if (o.line) scene.remove(o.line); }} fieldObjects.clear(); }}
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
    if (!(hi>lo)) return;
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
      txt += fl.name+' 1D sine |u|max='+hi.toFixed(3)+'<br>';
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
    txt += fl.name+' '+W+'\u00d7'+H+dk+'  E\u221d|u|max '+peak.toFixed(3)+'<br>';
  }});
  for (const [name, ent] of [...fieldObjects]) {{
    if (!seen.has(name)) {{ if (ent.mc) {{ scene.remove(ent.mc); scene.remove(ent.trough.mc); }} if (ent.line) scene.remove(ent.line); fieldObjects.delete(name); }}
  }}
  return txt;
}}
function applyFrame(f) {{
  const isMol = f.bonds && f.bonds.length>0;
  for (const m of meshes.values()) scene.remove(m);
  meshes.clear();
  for (const m of decals) scene.remove(m); decals.length=0;
  let html = '<b>t='+f.time.toFixed(3)+'</b><hr>';
  let sun=null;
  for (const e of f.entities) {{ const r=Math.hypot(e.pos[0],e.pos[1]); if(!sun||r<sun.r) sun={{r:r,x:e.pos[0],y:e.pos[1],z:e.pos[2]}}; }}
  if (sun) sunLight.position.set(sun.x,sun.y,sun.z);
  for (const e of f.entities) {{
    const m = makeMesh(e);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);
    m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if ((e.name==='sun'||(sun&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6)) && m.material) {{ m.material.emissive=new THREE.Color(e.color); if (e.glow==null) m.material.emissiveIntensity=1.2; }}
    m.userData=e; scene.add(m); meshes.set(e.id, m);
    if (e.name && e.label!==false && labelsOn) {{ const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name; const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l); }}
    if (!isMol && sun && e.state && e.state.length>=5 && Math.hypot(e.state[3],e.state[4],e.state[5])>1e-6) addOrbit(sun, Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y), e.color);
    if (!isMol && e.state && e.state.length>=5) addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);
    if (e.state && e.state.length) html += (e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
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
  const selE = selected ? f.entities.find(e=>e===selected.userData) : null;
  if (selE) {{ highlight.position.set(selE.pos[0],selE.pos[1],selE.pos[2]); highlight.visible=true; }}
  else highlight.visible=false;
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
  controls.update(); renderer.render(scene, camera); labelRenderer.render(scene, camera);
}});
addEventListener('resize', ()=>{{ camera.aspect=innerWidth/innerHeight; camera.updateProjectionMatrix(); renderer.setSize(innerWidth,innerHeight); labelRenderer.setSize(innerWidth,innerHeight); }});
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
        let live = state.read().unwrap();
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
        out.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
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
const scene=new THREE.Scene(); scene.background=new THREE.Color(0x0b0e14);
const gridHelp=new THREE.GridHelper(20,20,0x2a3240,0x1a2030); scene.add(gridHelp); const axesHelp=new THREE.AxesHelper(2); scene.add(axesHelp);
scene.add(new THREE.AmbientLight(0xffffff,0.5)); scene.add(new THREE.HemisphereLight(0x9cc4ff,0x0b0e14,0.9)); const dl=new THREE.DirectionalLight(0xffffff,0.8); dl.position.set(8,14,10); scene.add(dl);
const sunLight=new THREE.PointLight(0xFFD24A,2,100); scene.add(sunLight);
const camera=new THREE.PerspectiveCamera(60,innerWidth/innerHeight,0.01,1000); camera.position.set(8,8,8);
const renderer=new THREE.WebGLRenderer({antialias:true}); renderer.setSize(innerWidth,innerHeight);
document.body.appendChild(renderer.domElement);
const labelRenderer=new CSS2DRenderer(); labelRenderer.setSize(innerWidth,innerHeight); labelRenderer.domElement.style.position='absolute'; labelRenderer.domElement.style.top='0'; labelRenderer.domElement.style.pointerEvents='none'; document.body.appendChild(labelRenderer.domElement);
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
const highlight=new THREE.Mesh(new THREE.SphereGeometry(0.4,16,16),new THREE.MeshBasicMaterial({color:0xffffff,wireframe:true,transparent:true,opacity:0.5}));highlight.visible=false;scene.add(highlight);
let selected=null;const inspect=document.createElement('div');inspect.style.cssText='position:fixed;left:8px;bottom:56px;background:#11141c;border:1px solid #2a3240;padding:8px;font-size:11px;z-index:10;max-width:300px;';document.body.appendChild(inspect);
renderer.domElement.addEventListener('pointerdown',(ev)=>{
  const rect=renderer.domElement.getBoundingClientRect();
  pointer.x=((ev.clientX-rect.left)/rect.width)*2-1; pointer.y=-((ev.clientY-rect.top)/rect.height)*2+1;
  raycaster.setFromCamera(pointer,camera);
  const hits=raycaster.intersectObjects([...meshes.values()]);
  selected=hits.length?hits[0].object:null;
});
const panel=document.getElementById('panel'), procEl=document.getElementById('proc'), conn=document.getElementById('conn');
const meshes=new Map();
function svgGeo(d, depth, scale){ const data=new SVGLoader().parse(d); let sh=[]; for(const p of data.paths) sh=sh.concat(SVGLoader.createShapes(p));
  const geo=new THREE.ExtrudeGeometry(sh,{depth:Math.max(depth,0.001),bevelEnabled:false,curveSegments:16}); geo.scale(scale,-scale,scale); geo.center(); return geo; }
function polyGeo(pts, faces){ const pos=[]; const F=faces&&faces.length?faces:null;
  if(F){ for(const f of F){ for(let i=1;i+1<f.length;i++){ for(const q of [f[0],f[i],f[i+1]]){ const p=pts[q]; pos.push(p[0],p[1],p[2]); } } } }
  const g=new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos,3)); g.computeVertexNormals(); return g; }
function capsuleGeo(r0,len,r1){ const cs=8, pts=[];
  for(let i=0;i<=cs;i++){const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r0*Math.sin(a), -len/2 - r0*Math.cos(a)));}
  for(let i=0;i<=cs;i++){const a=i/cs*Math.PI/2; pts.push(new THREE.Vector2(r1*Math.cos(a), len/2 + r1*Math.sin(a)));}
  return new THREE.LatheGeometry(pts,28); }
function make(e){
  const opacity=e.opacity==null?1:e.opacity, glow=e.glow==null?0.8:e.glow;
  const mat=new THREE.MeshStandardMaterial({color:e.color,emissive:new THREE.Color(e.color),emissiveIntensity:glow,metalness:0.0,roughness:0.5,transparent:opacity<1,opacity:opacity});
  if(e.kind==='box') return new THREE.Mesh(new THREE.BoxGeometry(e.dims[0],e.dims[1],e.dims[2]),mat);
  if(e.kind==='sphere') return new THREE.Mesh(new THREE.SphereGeometry(e.radius,20,16),mat);
  if(e.kind==='hull'&&e.points){const v=e.points.map(p=>new THREE.Vector3(p[0],p[1],p[2]));let g;try{g=new ConvexGeometry(v);}catch(err){g=new THREE.SphereGeometry(0.1,8,6);}return new THREE.Mesh(g,mat);}
  if(e.kind==='capsule') return new THREE.Mesh(capsuleGeo(e.r0||0.06, e.len||0.3, e.r1||0.06), mat);
  if(e.kind==='group' && e.parts){ const g=new THREE.Group(); for(const p of e.parts){ let ch;
    if(p.k===2){ ch=new THREE.Mesh(new THREE.BoxGeometry(p.a,p.b,p.c), mat); }
    else if(p.k===3){ ch=new THREE.Mesh(capsuleGeo(p.a,p.b,p.c), mat); }
    else if(p.k===4){ ch=new THREE.Mesh(svgGeo(p.d, p.depth, p.scale), mat); }
    else if(p.k===5){ ch=new THREE.Mesh(new ConvexGeometry((p.pts||[]).map(q=>new THREE.Vector3(q[0],q[1],q[2]))), mat); }
    else if(p.k===6){ ch=new THREE.Mesh(polyGeo(p.pts||[], p.faces||[]), mat); }
    else { ch=new THREE.Mesh(new THREE.SphereGeometry(p.a,20,16), mat); }
    ch.position.set(p.off[0],p.off[1],p.off[2]); g.add(ch); } return g; }
  return new THREE.Mesh(new THREE.SphereGeometry((e.size||0.25)/2,20,16),mat);
}
const fieldObjects = new Map();
let fieldExtent = 0;
function clearFields() { for (const o of fieldObjects.values()) { if (o.mc) { scene.remove(o.mc); scene.remove(o.trough.mc); } if (o.line) scene.remove(o.line); if (o.pts) scene.remove(o.pts); } fieldObjects.clear(); }
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
    if (!(hi>lo)) return;
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
      txt += fl.name+' 1D sine |u|max='+hi.toFixed(3)+'<br>';
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
    txt += fl.name+' '+W+'\u00d7'+H+dk+'  E\u221d|u|max '+peak.toFixed(3)+'<br>';
  });
  for (const [name, ent] of [...fieldObjects]) {
    if (!seen.has(name)) { if (ent.mc) { scene.remove(ent.mc); scene.remove(ent.trough.mc); } if (ent.line) scene.remove(ent.line); fieldObjects.delete(name); }
  }
  return txt;
}
function apply(f){
  const isMol = f.frame.bonds && f.frame.bonds.length > 0;
  for(const m of meshes.values()) scene.remove(m); meshes.clear();
  for(const m of decals) scene.remove(m); decals.length=0;
  let html='<b>step '+f.step+' · t='+f.frame.time.toFixed(3)+'</b><hr>';
  // Sun = the body nearest the origin (central body).
  let sun={x:0,y:0,z:0};
  for(const e of f.frame.entities){const r=Math.hypot(e.pos[0],e.pos[1]);if(!sun.r||r<sun.r){sun.r=r;sun.x=e.pos[0];sun.y=e.pos[1];sun.z=e.pos[2];}}
  sunLight.position.set(sun.x,sun.y,sun.z);
  for(const e of f.frame.entities){
    const m=make(e);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if((e.name==='sun'||(sun.r&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6))&&m.material){m.material.emissive=new THREE.Color(e.color);if(e.glow==null)m.material.emissiveIntensity=0.6;}
    m.userData=e; scene.add(m);meshes.set(e.id,m);
    // Name label.
    if(e.label!==false && labelsOn){ const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name||('#'+e.id);
    const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l); }
    // For a molecule (bonds present) skip orbit rings; atoms don't orbit.
    if(!isMol && e.state && e.state.length>=5 && Math.hypot(e.state[3],e.state[4],e.state[5])>1e-6){ if(sun.r){const r=Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y);addOrbit(sun,r,e.color);} }
    // Velocity vector (skip for static molecule atoms).
    if(!isMol && e.state&&e.state.length>=5){addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);}
    html+='<span style="color:#'+e.color.toString(16).padStart(6,'0')+'">■</span> '+(e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
  }
  // Draw bonds (molecule) as lines between bonded atoms.
  if(isMol) addBonds(f.frame);
  const hasFields=f.frame.fields&&f.frame.fields.length; gridHelp.visible=!hasFields; axesHelp.visible=!hasFields;
  if(hasFields) html+='<hr>'+renderFields(f.frame.fields);
  for(const c of f.frame.channels) html+='ch#'+c.id+' = '+c.value.toFixed(3)+'<br>';
  panel.innerHTML=html;
  let p=''; for(let i=f.info.length-1;i>=0;i--) p+=f.info[i]+'<br>'; procEl.innerHTML=p;
  if(f.frame.camera&&!cameraInit){camera.position.set(f.frame.camera.pos[0],f.frame.camera.pos[1],f.frame.camera.pos[2]);camera.lookAt(f.frame.camera.target[0],f.frame.camera.target[1],f.frame.camera.target[2]);cameraInit=true;}
  else if(fieldExtent>0&&!cameraInit){const e=fieldExtent*2.0;camera.position.set(e,e*0.8,e);camera.lookAt(0,0,0);controls.target.set(0,0,0);controls.update();cameraInit=true;}
  // Highlight + inspect the selected body.
  lastFrame=f;
  const selE=selected?f.frame.entities.find(e=>e===selected.userData):null;
  if(selE){highlight.position.set(selE.pos[0],selE.pos[1],selE.pos[2]);highlight.visible=true;
    const st=selE.state||[];inspect.innerHTML='<b>'+selE.name+'</b> ('+selE.kind+')<br>pos '+selE.pos.map(x=>x.toFixed(2)).join(', ')+'<br>'+(st.length?'state ['+st.map(x=>x.toFixed(3)).join(', ')+']':'')+'<br>size '+ (selE.size||0.25).toFixed(2);
  } else {highlight.visible=false;inspect.innerHTML='';}
}
let lastFrame=null;
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
  for(const [a,b] of frame.bonds){
    const A=meshes.get(a),B=meshes.get(b); if(!A||!B) continue;
    const p1=A.position,p2=B.position;
    const dir=p2.clone().sub(p1); const len=dir.length(); if(len<1e-6) continue;
    const m=new THREE.Mesh(new THREE.CylinderGeometry(0.06,0.06,len,8,1,true),
      new THREE.MeshPhongMaterial({color:0xcccccc,transparent:true,opacity:0.9}));
    m.position.copy(p1).add(p2).multiplyScalar(0.5);
    m.quaternion.setFromUnitVectors(new THREE.Vector3(0,1,0),dir.clone().normalize());
    scene.add(m);decals.push(m);
  }
}
let alive=true;
addEventListener('pagehide',()=>{alive=false;});
addEventListener('beforeunload',()=>{alive=false;});
async function poll(){
  if(!alive) return;
  try{const r=await fetch('/state',{cache:'no-store'});const f=await r.json();apply(f);conn.style.display='none';}
  catch(e){conn.style.display='block';conn.textContent='waiting for runtime…';}
  if(alive) setTimeout(poll,60);
}
document.getElementById('rst').onclick=()=>{fetch('/reset').catch(()=>{});};
let paused=false;
document.getElementById('pse').onclick=()=>{paused=!paused;fetch('/pause?on='+(paused?1:0)).then(r=>r.text()).then(()=>{document.getElementById('pse').textContent=(paused?'▶ Resume':'⏸ Pause');}).catch(()=>{});};
document.getElementById('rst').addEventListener('click',()=>{paused=true;document.getElementById('pse').textContent='▶ Resume';});
document.getElementById('lbl').onclick=()=>{labelsOn=!labelsOn;document.getElementById('lbl').style.opacity=labelsOn?'1':'0.45';};
poll();
renderer.setAnimationLoop(()=>{controls.update();renderer.render(scene,camera);labelRenderer.render(scene,camera);});
addEventListener('resize',()=>{camera.aspect=innerWidth/innerHeight;camera.updateProjectionMatrix();renderer.setSize(innerWidth,innerHeight);labelRenderer.setSize(innerWidth,innerHeight);});
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
        ] {
            assert!(vendor_file(f).is_some(), "missing vendored asset {f}");
        }
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
            shape_name: Some("gizmo".into()),
            parts: Some(vec![
                crate::components::ShapePart {
                    kind: 3,
                    a: 0.05,
                    b: 0.3,
                    c: 0.05,
                    offset: (0.0, 0.0, 0.0),
                    path: None,
                    points: Vec::new(),
                    faces: Vec::new(),
                    scale: 1.0,
                },
                crate::components::ShapePart {
                    kind: 1,
                    a: 0.1,
                    b: 0.1,
                    c: 0.1,
                    offset: (0.0, 0.2, 0.0),
                    path: None,
                    points: Vec::new(),
                    faces: Vec::new(),
                    scale: 1.0,
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
        assert!(html.contains("three@0.160.0"));
        // The frames placeholder is substituted.
        assert!(html.contains(&format!("[{json}]")));
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
        assert!(page.contains("fetch('/state')"));
        assert!(page.contains("OrbitControls"));
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
