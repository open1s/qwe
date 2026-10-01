//! Hand-rolled throughput benchmarks (no external deps). Run:
//! `cargo bench -p pwe-reference`. Each line is a stable, comparable number so
//! performance regressions are visible in CI; compare with
//! `tools/bench-check.sh` (local thresholds) or `tools/bench-check.sh --ci`
//! (cross-machine ceilings against `benches/baseline.txt`).
use pwe_reference::lang::LangRuntime;
use pwe_reference::native::NativeProgram;
use pwe_reference::physics_eir::SceneRuntime;
use std::time::Instant;

fn nbody_src(n: usize) -> String {
    let mut s = String::from("world { gravity=(0,0,0)\n");
    for i in 0..n {
        let a = i as f64 * 0.37;
        s += &format!(
            "  entity b{i} {{ state = ({:.4}, {:.4}, 0, 0, 0, 0, 1.0) }}\n",
            a.cos() * 5.0,
            a.sin() * 5.0
        );
    }
    s += "}\nsystems { nbody { G = 0.001; dt = 0.001 } }\n";
    s
}

const FIELD_SRC: &str = r#"
world { gravity=(0,0,0)
  field u  { width = 32; height = 32; dx = 1.0 }
  field um { width = 32; height = 32; dx = 1.0 }
  entity probe { state = (0.0, 0.0, 0.0) }
}
systems {
  wave { field = u; prev = um; velocity = 0.2; dt = 0.1 }
  update { on = probe; dt = 0.1 s0 = s0 + inte( 0.0 ) }
}
"#;

/// Iteration scale for wall-clock-short runs (pre-commit / CI). The per-op
/// number is unaffected: warm-up happens before the timer starts, so scaling
/// only changes how many timed iterations are averaged.
fn bench_scale() -> u64 {
    std::env::var("PWE_BENCH_SCALE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1)
        .max(1)
}

fn timed<F: FnMut()>(label: &str, iters: u64, mut f: F) {
    let iters = (iters / bench_scale()).max(10);
    // Warm-up is outside the timed region; keep it substantial even at a high
    // scale so one-off first-call costs cannot skew a short run.
    for _ in 0..(iters / 10).max(5) {
        f();
    }
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
    println!("{label:<28} {us:>10.3} us/op");
}

fn main() {
    let nbody = nbody_src(64);

    timed("compile(nbody 64)", 50, || {
        let _ = LangRuntime::compile(&nbody).unwrap();
    });
    // Sub-millisecond: take many iterations so the per-op average averages out
    // host noise (the timed window would otherwise be only a few ms).
    timed("compile(field 32x32)", 2_000, || {
        let _ = LangRuntime::compile(FIELD_SRC).unwrap();
    });

    let mut rt = LangRuntime::compile(&nbody).unwrap();
    timed("step_interpreter(nbody 64)", 20_000, || {
        rt.step_interpreter().unwrap();
    });

    // Opt-in threaded-dispatch interpreter vs the default jump table.
    let mut rt_t = LangRuntime::compile(&nbody).unwrap();
    rt_t.enable_native_jit(false);
    rt_t.enable_threaded_dispatch(true);
    timed("step_threaded(nbody 64)", 20_000, || {
        rt_t.step_interpreter().unwrap();
    });
    timed("step_cross(nbody 64)", 5_000, || {
        rt.step_cross().unwrap();
    });
    timed("present_frame(nbody 64)", 2_000, || {
        let _ = rt.present_frame(None);
    });

    // Phase-3 native backend: pure world kernel compiled to machine code with
    // `cc -O2` (or skipped when no C compiler is available).
    if let Ok(native) = NativeProgram::compile(&rt.module) {
        timed("step_native(nbody 64)", 20_000, || {
            let mut r = SceneRuntime::new(&rt.scene);
            let _ = native.execute_entries(&rt.module, &mut r).unwrap();
        });
    } else {
        println!(
            "{:<28} (no `cc`; native backend skipped)",
            "step_native(nbody 64)"
        );
    }

    let mut rtf = LangRuntime::compile(FIELD_SRC).unwrap();
    timed("step_interpreter(wave 32x32)", 20_000, || {
        rtf.step_interpreter().unwrap();
    });

    println!("(field = 32x32 = {} cells/step)", 32 * 32);

    #[cfg(all(feature = "gpu", target_os = "macos"))]
    bench_gpu();
}

// Phase-3 GPU (Metal) record: GPU field-sweep offload vs the CPU stencil. The
// backend is an *experimental* correctness-verified f32 offload; this measures
// its crossover. Built only with `--features gpu` on macOS.
#[cfg(all(feature = "gpu", target_os = "macos"))]
fn bench_gpu() {
    fn cpu_diffuse(
        cur: &[f64],
        out: &mut [f64],
        w: usize,
        h: usize,
        d: usize,
        rate: f64,
        dx2: f64,
    ) {
        for k in 0..d {
            for j in 0..h {
                for i in 0..w {
                    let idx = (k * h + j) * w + i;
                    let c = cur[idx];
                    let at = |ci: usize, cj: usize, ck: usize| cur[(ck * h + cj) * w + ci];
                    let l = if i > 0 { at(i - 1, j, k) } else { c };
                    let r = if i + 1 < w { at(i + 1, j, k) } else { c };
                    let u = if j > 0 { at(i, j - 1, k) } else { c };
                    let dn = if j + 1 < h { at(i, j + 1, k) } else { c };
                    let b = if k > 0 { at(i, j, k - 1) } else { c };
                    let f = if k + 1 < d { at(i, j, k + 1) } else { c };
                    out[idx] = c + rate * ((l + r + u + dn + b + f - 6.0 * c) / dx2);
                }
            }
        }
    }
    let gpu = match pwe_reference::gpu::Gpu::new() {
        Ok(g) => g,
        Err(_) => {
            println!("{:<28} (no Metal device)", "gpu_diffuse");
            return;
        }
    };
    println!("GPU device: {}", gpu.name());
    let (w, h, d) = (256usize, 256usize, 1usize);
    let n = w * h * d;
    let cur: Vec<f64> = (0..n).map(|i| ((i as f64) * 0.001).sin()).collect();
    let mut out = vec![0.0f64; n];
    let dx2 = 1.0 / 64.0;
    timed("cpu_diffuse(256x256)", 200, || {
        cpu_diffuse(&cur, &mut out, w, h, d, 0.2, dx2);
    });
    timed("gpu_diffuse(256x256)", 200, || {
        out = gpu.diffuse(&cur, w, h, d, 0.2, dx2).unwrap();
    });
    std::hint::black_box(&out);
}
