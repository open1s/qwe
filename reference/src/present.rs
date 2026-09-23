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

/// An immutable presentation snapshot of one world state.
#[derive(Clone, Debug, Default)]
pub struct PresentationFrame {
    pub time: f64,
    pub entities: Vec<EntityVisual>,
    pub channels: Vec<ChannelVisual>,
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
        entities.push(EntityVisual {
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
        });
    }
    entities.sort_by_key(|e| e.id);
    channel_list.sort_by_key(|c| c.id);
    PresentationFrame {
        time: scene.sim_time,
        entities,
        channels: channel_list,
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
            "{{\"id\":{},\"name\":\"{}\",\"pos\":{},\"rot\":{},\"color\":{},\"kind\":",
            e.id,
            e.name.replace('\\', "\\\\").replace('"', "\\\""),
            vec3_json(e.position),
            quat_json(e.rotation),
            e.color_value()
        ));
        match &e.shape {
            Shape::Box { dims } => {
                out.push_str(&format!("\"box\",\"dims\":{}}}", vec3_json(*dims)));
            }
            Shape::Sphere { radius } => {
                out.push_str(&format!("\"sphere\",\"radius\":{}}}", fmt_f64(*radius)));
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
    out.push_str(",\"bonds\":[");
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
</div>
<script type="importmap">{{
  "imports": {{
    "three": "https://unpkg.com/three@0.160.0/build/three.module.js",
    "three/addons/": "https://unpkg.com/three@0.160.0/examples/jsm/"
  }}
}}</script>
<script type="module">
import * as THREE from 'three';
import {{ OrbitControls }} from 'three/addons/controls/OrbitControls.js';
import {{ ConvexGeometry }} from 'three/addons/geometries/ConvexGeometry.js';
import {{ CSS2DRenderer, CSS2DObject }} from 'three/addons/renderers/CSS2DRenderer.js';
const FRAMES = {frames_json};
let idx = 0, playing = false;
const scene = new THREE.Scene();
scene.background = new THREE.Color(0x0b0e14);
scene.add(new THREE.GridHelper(20, 20, 0x2a3240, 0x1a2030));
scene.add(new THREE.AxesHelper(2));
scene.add(new THREE.AmbientLight(0xffffff, 0.5));
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
function makeMesh(kind, dims, radius, points, color, size) {{
  const mat = new THREE.MeshStandardMaterial({{color: color,emissive:new THREE.Color(color),emissiveIntensity:0.8,metalness:0.0,roughness:0.5}});
  if (kind==='box') return new THREE.Mesh(new THREE.BoxGeometry(dims[0],dims[1],dims[2]), mat);
  if (kind==='sphere') return new THREE.Mesh(new THREE.SphereGeometry(radius,20,16), mat);
  if (kind==='hull' && points) {{
    const verts = points.map(p=>new THREE.Vector3(p[0],p[1],p[2]));
    let g; try {{ g = new ConvexGeometry(verts); }} catch(e) {{ g = new THREE.SphereGeometry(0.1,8,6); }}
    return new THREE.Mesh(g, mat);
  }}
  return new THREE.Mesh(new THREE.SphereGeometry(size/2,20,16), mat);
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
    const m = makeMesh(e.kind, e.dims, e.radius, e.points, e.color, e.size || 0.25);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);
    m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if (e.name==='sun'||(sun&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6)) {{ m.material.emissive=new THREE.Color(e.color); m.material.emissiveIntensity=1.2; }}
    m.userData=e; scene.add(m); meshes.set(e.id, m);
    if (e.name) {{ const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name; const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l); }}
    if (!isMol && sun) addOrbit(sun, Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y), e.color);
    if (!isMol && e.state && e.state.length>=5) addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);
    if (e.state && e.state.length) html += (e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
  }}
  if (isMol) addBonds(f);
  for (const c of f.channels) html += 'ch#'+c.id+' = '+c.value.toFixed(3)+'<br>';
  panel.innerHTML = html;
  if (f.camera && !cameraSet) {{ camera.position.set(f.camera.pos[0],f.camera.pos[1],f.camera.pos[2]); camera.lookAt(f.camera.target[0],f.camera.target[1],f.camera.target[2]); cameraSet=true; }}
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
pub fn serve_live(state: Arc<RwLock<LiveState>>, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let page = live_viewer_html();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            let page = page.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                let _ = handle_connection(&mut stream, &state, &page);
            });
        }
    });
    Ok(())
}

fn handle_connection(
    stream: &mut TcpStream,
    state: &Arc<RwLock<LiveState>>,
    page: &str,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_millis(2000)))?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

    let (status, content_type, body) = if path == "/" {
        ("200 OK", "text/html", page.as_bytes().to_vec())
    } else if path == "/state" {
        let live = state.read().unwrap();
        let body = live_state_json(&live);
        ("200 OK", "application/json", body.into_bytes())
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
<script type="importmap">{"imports":{
  "three":"https://unpkg.com/three@0.160.0/build/three.module.js",
  "three/addons/":"https://unpkg.com/three@0.160.0/examples/jsm/"
}}</script>
<script type="module">
import * as THREE from 'three';
import {OrbitControls} from 'three/addons/controls/OrbitControls.js';
import {ConvexGeometry} from 'three/addons/geometries/ConvexGeometry.js';
import {CSS2DRenderer,CSS2DObject} from 'three/addons/renderers/CSS2DRenderer.js';
const scene=new THREE.Scene(); scene.background=new THREE.Color(0x0b0e14);
scene.add(new THREE.GridHelper(20,20,0x2a3240,0x1a2030)); scene.add(new THREE.AxesHelper(2));
scene.add(new THREE.AmbientLight(0xffffff,0.5)); const dl=new THREE.DirectionalLight(0xffffff,0.8); dl.position.set(8,14,10); scene.add(dl);
const sunLight=new THREE.PointLight(0xFFD24A,2,100); scene.add(sunLight);
const camera=new THREE.PerspectiveCamera(60,innerWidth/innerHeight,0.01,1000); camera.position.set(8,8,8);
const renderer=new THREE.WebGLRenderer({antialias:true}); renderer.setSize(innerWidth,innerHeight);
document.body.appendChild(renderer.domElement);
const labelRenderer=new CSS2DRenderer(); labelRenderer.setSize(innerWidth,innerHeight); labelRenderer.domElement.style.position='absolute'; labelRenderer.domElement.style.top='0'; labelRenderer.domElement.style.pointerEvents='none'; document.body.appendChild(labelRenderer.domElement);
const controls=new OrbitControls(camera,renderer.domElement);
controls.enableDamping=true; controls.dampingFactor=0.08; controls.autoRotate=true; controls.autoRotateSpeed=1.4;
controls.minDistance=0.5; controls.maxDistance=250; controls.screenSpacePanning=true; controls.maxPolarAngle=Math.PI;
let cameraInit=false;
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
function make(kind,dims,radius,points,color,size){
  const mat=new THREE.MeshStandardMaterial({color:color});
  if(kind==='box') return new THREE.Mesh(new THREE.BoxGeometry(dims[0],dims[1],dims[2]),mat);
  if(kind==='sphere') return new THREE.Mesh(new THREE.SphereGeometry(radius,20,16),mat);
  if(kind==='hull'&&points){const v=points.map(p=>new THREE.Vector3(p[0],p[1],p[2]));let g;try{g=new ConvexGeometry(v);}catch(e){g=new THREE.SphereGeometry(0.1,8,6);}return new THREE.Mesh(g,mat);}
  return new THREE.Mesh(new THREE.SphereGeometry(size/2,20,16),mat);
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
    const m=make(e.kind,e.dims,e.radius,e.points,e.color,e.size||0.25);
    m.position.set(e.pos[0],e.pos[1],e.pos[2]);m.quaternion.set(e.rot[0],e.rot[1],e.rot[2],e.rot[3]);
    if(e.name==='sun'||(sun.r&&Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y)<1e-6)){m.material.emissive=new THREE.Color(e.color);m.material.emissiveIntensity=0.6;}
    m.userData=e; scene.add(m);meshes.set(e.id,m);
    // Name label.
    const el=document.createElement('div'); el.className='lbl'; el.textContent=e.name||('#'+e.id);
    const l=new CSS2DObject(el); l.position.set(e.pos[0],e.pos[1]+(e.size||0.3),e.pos[2]); scene.add(l); decals.push(l);
    // For a molecule (bonds present) skip orbit rings; atoms don't orbit.
    if(!isMol){ if(sun.r){const r=Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y);addOrbit(sun,r,e.color);} }
    // Velocity vector (skip for static molecule atoms).
    if(!isMol && e.state&&e.state.length>=5){addVel(e.pos[0],e.pos[1],e.pos[2],e.state[3],e.state[4],e.state[5],e.color);}
    html+='<span style="color:#'+e.color.toString(16).padStart(6,'0')+'">■</span> '+(e.name||('#'+e.id))+' r='+Math.hypot(e.pos[0]-sun.x,e.pos[1]-sun.y).toFixed(2)+'<br>';
  }
  // Draw bonds (molecule) as lines between bonded atoms.
  if(isMol) addBonds(f.frame);
  for(const c of f.frame.channels) html+='ch#'+c.id+' = '+c.value.toFixed(3)+'<br>';
  panel.innerHTML=html;
  let p=''; for(let i=f.info.length-1;i>=0;i--) p+=f.info[i]+'<br>'; procEl.innerHTML=p;
  if(f.frame.camera&&!cameraInit){camera.position.set(f.frame.camera.pos[0],f.frame.camera.pos[1],f.frame.camera.pos[2]);camera.lookAt(f.frame.camera.target[0],f.frame.camera.target[1],f.frame.camera.target[2]);cameraInit=true;}
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
async function poll(){
  try{const r=await fetch('/state');const f=await r.json();apply(f);conn.style.display='none';}
  catch(e){conn.style.display='block';conn.textContent='waiting for runtime…';}
  setTimeout(poll,60);
}
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
