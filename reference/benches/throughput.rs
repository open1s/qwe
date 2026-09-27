//! Hand-rolled throughput benchmarks (no external deps). Run:
//! `cargo bench -p pwe-reference`. Each line is a stable, comparable number so
//! performance regressions are visible in CI.
use pwe_reference::lang::LangRuntime;
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

fn timed<F: FnMut()>(label: &str, iters: u64, mut f: F) {
    for _ in 0..(iters / 10).max(1) {
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
    timed("compile(field 32x32)", 50, || {
        let _ = LangRuntime::compile(FIELD_SRC).unwrap();
    });

    let mut rt = LangRuntime::compile(&nbody).unwrap();
    timed("step_interpreter(nbody 64)", 20_000, || {
        rt.step_interpreter().unwrap();
    });
    timed("step_cross(nbody 64)", 5_000, || {
        rt.step_cross().unwrap();
    });
    timed("present_frame(nbody 64)", 2_000, || {
        let _ = rt.present_frame(None);
    });

    let mut rtf = LangRuntime::compile(FIELD_SRC).unwrap();
    timed("step_interpreter(wave 32x32)", 20_000, || {
        rtf.step_interpreter().unwrap();
    });

    println!("(field = 32x32 = {} cells/step)", 32 * 32);
}
