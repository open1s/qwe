//! `pwe` — the PWE command-line toolchain.
//!
//! Like `javac`/`java` for the PWE language:
//!
//! * `pwe compile <src.pwe> [-o <out.pweb>]` — compile a source program into a
//!   self-describing `.pweb` artifact: the verified canonical (RFC-0021) EIR
//!   module plus the world-model source the runtime derives its initial scene
//!   from. Compile failures render the source with a caret.
//! * `pwe run <out.pweb> [--steps N]` — execute the compiled binary
//!   deterministically (interpreter == JIT, cross-checked every step).
//! * `pwe present <out.pweb> [--port P]` — execute it live and serve the
//!   browser 3D viewer.
//!
//! Exit codes: 0 success, 1 compile/runtime failure, 2 usage error.

use pwe_api::RegionId;
use pwe_reference::dsl::WorldModel;
use pwe_reference::eir::EirModule;
use pwe_reference::lang::{self, CompiledProgram, LangRuntime, ProgramSources};
use pwe_reference::math::Vec3;
use pwe_reference::physics_eir::PhysicsProgram;
use pwe_reference::present::{self, CameraVisual, LiveState};
use pwe_reference::sha256::digest;
use std::sync::{Arc, RwLock};

/// Artifact container magic and format version.
const MAGIC: &[u8; 4] = b"PWEB";
const VERSION: u16 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("compile") => cmd_compile(&args[1..]),
        Some("run") => cmd_run(&args[1..], None),
        Some("present") => cmd_run(&args[1..], Some(8000)),
        Some("-h") | Some("--help") | None => {
            usage();
            0
        }
        Some(other) => {
            eprintln!("pwe: unknown command '{other}'\n");
            usage();
            2
        }
    };
    std::process::exit(code);
}

fn usage() {
    println!(
        "pwe — the PWE language toolchain\n\
         \n\
         USAGE:\n  \
           pwe compile <src.pwe> [-o <out.pweb>]\n  \
           pwe run     <out.pweb> [--steps N] [--param K=V]...\n  \
           pwe present <out.pweb> [--port P] [--param K=V]...\n\
         \n\
         Compile source to a .pweb binary, then run the binary (javac/java style).\n\
         --param overrides a declared model parameter at run time.\n\
         A .pweb artifact holds the verified canonical EIR module plus the\n\
         world-model source it derives the initial scene from."
    );
}

// ---------------------------------------------------------------------------
// Artifact container
// ---------------------------------------------------------------------------

/// Packs a compiled module plus its (module-resolved) sources into the
/// `.pweb` container:
/// `magic | version | flags | eir_len | root_len | eir | root | modules… | aliases…`.
fn pack(eir: &EirModule, sources: &ProgramSources) -> Result<Vec<u8>, String> {
    let eir_bytes = eir
        .encode()
        .map_err(|e| format!("cannot encode EIR: {e}"))?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(eir_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&(sources.root.len() as u64).to_le_bytes());
    out.extend_from_slice(&eir_bytes);
    out.extend_from_slice(sources.root.as_bytes());
    out.extend_from_slice(&(sources.modules.len() as u32).to_le_bytes());
    for (ns, path, src, aliases) in &sources.modules {
        for part in [ns.as_str(), path.as_str(), src.as_str()] {
            out.extend_from_slice(&(part.len() as u32).to_le_bytes());
            out.extend_from_slice(part.as_bytes());
        }
        out.extend_from_slice(&(aliases.len() as u32).to_le_bytes());
        for a in aliases {
            out.extend_from_slice(&(a.len() as u32).to_le_bytes());
            out.extend_from_slice(a.as_bytes());
        }
    }
    out.extend_from_slice(&(sources.aliases.len() as u32).to_le_bytes());
    for (bare, qualified) in &sources.aliases {
        for part in [bare.as_str(), qualified.as_str()] {
            out.extend_from_slice(&(part.len() as u32).to_le_bytes());
            out.extend_from_slice(part.as_bytes());
        }
    }
    Ok(out)
}

fn take_u32(bytes: &[u8], cursor: &mut usize, what: &str) -> Result<u32, String> {
    let v = u32::from_le_bytes(
        bytes
            .get(*cursor..*cursor + 4)
            .ok_or_else(|| format!("truncated artifact: {what}"))?
            .try_into()
            .unwrap(),
    );
    *cursor += 4;
    Ok(v)
}

fn take_str<'a>(bytes: &'a [u8], cursor: &mut usize, what: &str) -> Result<&'a str, String> {
    let n = u32::from_le_bytes(
        bytes
            .get(*cursor..*cursor + 4)
            .ok_or_else(|| format!("truncated artifact: {what} length"))?
            .try_into()
            .unwrap(),
    ) as usize;
    *cursor += 4;
    let raw = bytes
        .get(*cursor..*cursor + n)
        .ok_or_else(|| format!("truncated artifact: {what}"))?;
    *cursor += n;
    std::str::from_utf8(raw).map_err(|_| format!("artifact {what} is not UTF-8"))
}

/// Unpacks a `.pweb` container, decoding and re-validating the EIR module.
fn unpack(bytes: &[u8]) -> Result<(EirModule, ProgramSources), String> {
    if bytes.len() < 24 || &bytes[..4] != MAGIC {
        return Err("not a .pweb artifact (bad magic)".to_string());
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(format!("unsupported .pweb version {version}"));
    }
    let eir_len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let root_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let eir_bytes = bytes
        .get(24..24 + eir_len)
        .ok_or("truncated artifact: EIR section")?;
    let root_bytes = bytes
        .get(24 + eir_len..24 + eir_len + root_len)
        .ok_or("truncated artifact: source section")?;
    let eir = EirModule::decode(eir_bytes).map_err(|e| format!("artifact EIR is invalid: {e}"))?;
    let root =
        String::from_utf8(root_bytes.to_vec()).map_err(|_| "artifact source is not UTF-8")?;
    let mut cursor = 24 + eir_len + root_len;
    let module_count = take_u32(bytes, &mut cursor, "module count")?;
    let mut modules = Vec::with_capacity(module_count as usize);
    for _ in 0..module_count {
        let ns = take_str(bytes, &mut cursor, "module namespace")?.to_string();
        let path = take_str(bytes, &mut cursor, "module path")?.to_string();
        let src = take_str(bytes, &mut cursor, "module source")?.to_string();
        let alias_count = take_u32(bytes, &mut cursor, "module alias count")?;
        let mut aliases = Vec::with_capacity(alias_count as usize);
        for _ in 0..alias_count {
            aliases.push(take_str(bytes, &mut cursor, "module alias")?.to_string());
        }
        modules.push((ns, path, src, aliases));
    }
    let alias_count = take_u32(bytes, &mut cursor, "alias count")?;
    let mut aliases = Vec::with_capacity(alias_count as usize);
    for _ in 0..alias_count {
        let bare = take_str(bytes, &mut cursor, "alias")?.to_string();
        let qualified = take_str(bytes, &mut cursor, "alias target")?.to_string();
        aliases.push((bare, qualified));
    }
    Ok((
        eir,
        ProgramSources {
            root,
            modules,
            aliases,
        },
    ))
}

/// Loads a runtime from a compiled `.pweb` artifact. Like `java` running a
/// `.class`, `run`/`present` execute the compiled binary — a source must be
/// compiled first with `pwe compile`.
fn load(path: &str) -> Result<(LangRuntime, WorldModel), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if !bytes.starts_with(MAGIC) {
        return Err(format!(
            "{path} is not a compiled .pweb artifact; compile it first:\n  pwe compile {path}"
        ));
    }
    // Execute the artifact's compiled EIR; the embedded sources only supply the
    // initial scene (entity layouts, gravity, fields, channels, parameters).
    let (eir, sources) = unpack(&bytes)?;
    let parsed = lang::merge_sources(&sources).map_err(|e| lang::diagnose(&sources.root, &e))?;
    let scene = parsed.model.build_scene();
    let program = PhysicsProgram {
        systems: Vec::new(),
        entities: Vec::new(),
        module: eir.clone(),
    };
    let compiled = CompiledProgram {
        parsed,
        program,
        eir,
    };
    let model = compiled.parsed.model.clone();
    let rt = LangRuntime::from_compiled_region(compiled, scene, RegionId(1))
        .map_err(|e| format!("cannot boot artifact {path}: {e}"))?;
    Ok((rt, model))
}

/// Appends `.pweb` to a path stem (`scene.pwe` -> `scene.pweb`).
fn with_extension(path: &str, ext: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.{ext}"),
        None => format!("{path}.{ext}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

fn cmd_compile(args: &[String]) -> i32 {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => {
                output = it.next().cloned();
                if output.is_none() {
                    eprintln!("pwe compile: -o needs a path");
                    return 2;
                }
            }
            other if other.starts_with('-') => {
                eprintln!("pwe compile: unknown option '{other}'");
                return 2;
            }
            other => {
                if input.is_some() {
                    eprintln!("pwe compile: unexpected extra argument '{other}'");
                    return 2;
                }
                input = Some(other.to_string());
            }
        }
    }
    let Some(input) = input else {
        eprintln!("pwe compile: missing <src.pwe>");
        return 2;
    };
    let output = output.unwrap_or_else(|| with_extension(&input, "pweb"));
    // Resolve Python-style `import` modules into one program.
    let (parsed, sources) = match lang::load_program_sources(std::path::Path::new(&input)) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{}", lang::diagnose("", &e));
            return 1;
        }
    };
    match lang::compile_program(parsed) {
        Ok(compiled) => {
            let bytes = match pack(&compiled.eir, &sources) {
                Ok(b) => b,
                Err(msg) => {
                    eprintln!("pwe: {msg}");
                    return 1;
                }
            };
            if let Err(e) = std::fs::write(&output, &bytes) {
                eprintln!("pwe: cannot write {output}: {e}");
                return 1;
            }
            println!("compiled {input} -> {output}");
            println!("  EIR functions: {}", compiled.eir.functions.len());
            println!("  artifact hash: {}", hex(&digest(&bytes).0));
            0
        }
        Err(e) => {
            eprintln!("{}", lang::diagnose(&sources.root, &e));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// run / present
// ---------------------------------------------------------------------------

fn cmd_run(args: &[String], present_default: Option<u16>) -> i32 {
    let mut input: Option<String> = None;
    let mut steps: u64 = 60;
    let mut port = present_default;
    let mut params: Vec<(String, f64)> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--steps" | "-n" => {
                let Some(v) = it.next().and_then(|s| s.parse::<u64>().ok()) else {
                    eprintln!("pwe: --steps needs a non-negative integer");
                    return 2;
                };
                steps = v;
            }
            "--param" | "-P" => {
                let Some(spec) = it.next() else {
                    eprintln!("pwe: --param needs NAME=VALUE");
                    return 2;
                };
                let Some((k, v)) = spec.split_once('=') else {
                    eprintln!("pwe: --param expects NAME=VALUE, got '{spec}'");
                    return 2;
                };
                let Some(v) = v.parse::<f64>().ok() else {
                    eprintln!("pwe: --param value must be a number, got '{v}'");
                    return 2;
                };
                params.push((k.to_string(), v));
            }
            "--port" | "-p" => {
                let Some(v) = it.next().and_then(|s| s.parse::<u16>().ok()) else {
                    eprintln!("pwe: --port needs a u16");
                    return 2;
                };
                port = Some(v);
            }
            other if other.starts_with('-') => {
                eprintln!("pwe: unknown option '{other}'");
                return 2;
            }
            other => {
                if input.is_some() {
                    eprintln!("pwe: unexpected extra argument '{other}'");
                    return 2;
                }
                input = Some(other.to_string());
            }
        }
    }
    let Some(input) = input else {
        eprintln!("pwe: missing <out.pweb> (compile a source first: pwe compile <src.pwe>)");
        return 2;
    };
    let (mut rt, model) = match load(&input) {
        Ok(pair) => pair,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    // Apply `--param` overrides (declared parameters only, so typos are caught).
    // A parameter imported under several namespaces has several alias keys;
    // update the whole group so every reference sees the new value.
    for (k, v) in &params {
        if !model.params.contains_key(k) && !model.param_alias.contains_key(k) {
            let mut declared: Vec<&str> = model.params.keys().map(String::as_str).collect();
            declared.sort();
            let list = if declared.is_empty() {
                "none".to_string()
            } else {
                declared.join(", ")
            };
            eprintln!("pwe: unknown parameter '{k}' (declared: {list})");
            return 2;
        }
        let canonical = model
            .param_alias
            .get(k)
            .cloned()
            .unwrap_or_else(|| k.clone());
        let group: Vec<String> = rt
            .scene
            .params
            .keys()
            .filter(|p| *p == &canonical || model.param_alias.get(*p) == Some(&canonical))
            .cloned()
            .collect();
        for p in group {
            rt.scene.params.insert(p, *v);
        }
    }
    match port {
        None => {
            for k in 0..steps {
                if let Err(e) = rt.step_cross() {
                    eprintln!("pwe: step {k} failed: {e}");
                    return 1;
                }
            }
            report(&rt, steps);
            0
        }
        Some(p) => present_live(rt, &model, p),
    }
}

/// Executes live and serves the browser viewer until interrupted.
fn present_live(mut rt: LangRuntime, model: &WorldModel, port: u16) -> i32 {
    let live = Arc::new(RwLock::new(LiveState::default()));
    // The viewer's Restart button sets this; the loop reloads the initial scene.
    let reset = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pause = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Err(e) = present::serve_live(
        Arc::clone(&live),
        Arc::clone(&reset),
        Arc::clone(&pause),
        port,
    ) {
        eprintln!("pwe: cannot serve on 127.0.0.1:{port}: {e}");
        return 1;
    }
    println!("pwe present: open http://localhost:{port}  (Ctrl-C to stop)");
    let cam = auto_frame_camera(&rt);
    let initial = rt.scene.clone();
    let mut step = 0u64;
    loop {
        if reset.swap(false, std::sync::atomic::Ordering::Relaxed) {
            rt.reset_to(initial.clone());
            step = 0;
        }
        if pause.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(16));
            continue;
        }
        if let Err(e) = rt.step_cross() {
            eprintln!("pwe: step {step} failed: {e}");
            return 1;
        }
        step += 1;
        let mut g = live.write().unwrap();
        g.step = step;
        g.frame = rt.present_frame(Some(cam));
        g.info = info_lines(&rt, model, step);
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

/// Composes the live run-info lines for the viewer's top-left panel: a compact
/// per-body radius summary (r from the central body, nearest the origin, as the
/// viewer computes it), an extra "from <parent>" distance for satellites
/// (`parent = <name>`), and the step + model title line at the bottom.
fn info_lines(rt: &LangRuntime, model: &WorldModel, step: u64) -> Vec<String> {
    let frame = rt.present_frame(None);
    let from_origin = |p: Vec3| (p.x * p.x + p.y * p.y + p.z * p.z).sqrt();
    let central = frame
        .entities
        .iter()
        .min_by(|a, b| {
            from_origin(a.position)
                .partial_cmp(&from_origin(b.position))
                .unwrap()
        })
        .map(|e| e.position);
    let by_name: std::collections::BTreeMap<String, Vec3> = frame
        .entities
        .iter()
        .map(|e| (e.name.clone(), e.position))
        .collect();
    let mut entries: Vec<String> = Vec::new();
    for e in &frame.entities {
        let r = central
            .map(|c| (e.position - c).length())
            .unwrap_or_else(|| from_origin(e.position));
        let parent = model
            .entities
            .get((e.id.saturating_sub(1)) as usize)
            .and_then(|d| d.parent.as_deref());
        match parent.and_then(|p| by_name.get(p).map(|pp| (p, *pp))) {
            Some((pname, ppos)) => entries.push(format!(
                "{}: {r:.2} from sun, {:.3} from {pname}",
                e.name,
                (e.position - ppos).length()
            )),
            None => entries.push(format!("{} {r:.2}", e.name)),
        }
    }
    let mut lines: Vec<String> = entries.chunks(5).map(|c| c.join("  ")).collect();
    let title = model.title.as_deref().unwrap_or("PWE run");
    lines.push(format!("step {step}: {title}"));
    // The viewer renders `info` bottom-up (its last element is the top line).
    lines.reverse();
    lines
}

/// Frames the camera on the scene's initial extent (a simple auto-fit).
fn auto_frame_camera(rt: &LangRuntime) -> CameraVisual {
    let frame = rt.present_frame(None);
    // Bounds of everything visible: entity positions plus every field's grid,
    // which is centered at the origin and spans ±(dims·dx)/2 per axis.
    let mut lo: Option<Vec3> = None;
    let mut hi: Option<Vec3> = None;
    let mut include = |p: Vec3| {
        lo = Some(match lo {
            Some(l) => Vec3::new(l.x.min(p.x), l.y.min(p.y), l.z.min(p.z)),
            None => p,
        });
        hi = Some(match hi {
            Some(h) => Vec3::new(h.x.max(p.x), h.y.max(p.y), h.z.max(p.z)),
            None => p,
        });
    };
    for e in &frame.entities {
        include(e.position);
    }
    for f in &frame.fields {
        let hx = f.width as f64 * f.dx / 2.0;
        let hy = f.height as f64 * f.dx / 2.0;
        let hz = f.depth as f64 * f.dx / 2.0;
        include(Vec3::new(-hx, -hy, -hz));
        include(Vec3::new(hx, hy, hz));
    }
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return CameraVisual {
            position: Vec3::new(2.0, 5.0, 30.0),
            target: Vec3::ZERO,
        };
    };
    let center = Vec3::new(
        (lo.x + hi.x) / 2.0,
        (lo.y + hi.y) / 2.0,
        (lo.z + hi.z) / 2.0,
    );
    let extent = (hi - lo).length().max(1.0);
    CameraVisual {
        position: center + Vec3::new(extent * 1.25, extent * 0.95, extent * 1.25),
        target: center,
    }
}

/// Reports the final state of a batch run.
fn report(rt: &LangRuntime, steps: u64) {
    println!("ran {steps} steps (interpreter == JIT, every step)");
    println!("  sim time:  {:.6} s", rt.scene.sim_time);
    println!("  entities:  {}", rt.scene.entities.len());
    for e in &rt.present_frame(None).entities {
        let p = e.position;
        print!(
            "  #{:<3} {:<14} pos = ({:9.4}, {:9.4}, {:9.4})",
            e.id, e.name, p.x, p.y, p.z
        );
        // Trim the state to its highest non-zero slot for readability.
        let last = e.state.iter().rposition(|v| *v != 0.0);
        if let Some(last) = last {
            let slots: Vec<String> = e.state[..=last].iter().map(|v| format!("{v:.4}")).collect();
            print!("  state = [{}]", slots.join(", "));
        }
        println!();
    }
    for (name, f) in &rt.scene.fields {
        if f.depth > 1 {
            println!(
                "  field {name}: {}x{}x{} dx={} total={:.6}",
                f.width,
                f.height,
                f.depth,
                f.dx,
                f.total()
            );
        } else {
            println!(
                "  field {name}: {}x{} dx={} total={:.6}",
                f.width,
                f.height,
                f.dx,
                f.total()
            );
        }
    }
    let events = rt.emitted_events();
    if !events.is_empty() {
        let kinds: Vec<String> = events.iter().map(|e| format!("{}", e.kind)).collect();
        println!("  events (last step): {}", kinds.join(", "));
    }
    for line in rt.logs() {
        println!("  {line}");
    }
}
