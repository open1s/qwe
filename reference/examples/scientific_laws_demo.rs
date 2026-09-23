//! **Scientific Laws gallery, live** — the PWE language as a general
//! simulation substrate for physical, chemical, and biological laws.
//!
//! Five independent law modules run **simultaneously**, each expressed in PWE
//! source and rendered live in one 3D scene. Bodies move / grow / shrink as
//! their law evolves, proving the language models, simulates, and visualizes
//! diverse scientific phenomena — not just rigid-body physics:
//!
//! | Module (x) | Law | Law expressed |
//! | --- | --- | --- |
//! | harmonic (−12) | Simple harmonic motion | `x'' = −ω²x` |
//! | kepler (−4)   | Keplerian orbit (Newton gravity) | `a = −GM r̂/r²` |
//! | chem (+4)     | Reversible kinetics | `A ⇌ B`, mass conserved |
//! | logistic (+12)| Logistic growth | `N' = rN(1−N/K)` |
//! | decay (+16)   | Radioactive decay | `N' = −λN` |
//! | thermo (+20)  | Bang-bang thermostat (branch control) | hysteresis 18–20 °C via `and`/`or`/`not` + `if` |
//!
//! `interpreter == JIT` is asserted every step.
//!
//! Run: `cargo run --example scientific_laws_demo [port]` then open the URL.

use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::{self, CameraVisual, LiveState};
use std::sync::{Arc, RwLock};

const SOURCE: &str = r#"
    world {
        gravity = (0, 0, 0)
        # --- Module 1: simple harmonic oscillator (x'' = -w^2 x), centre -12 ---
        entity hosc { state = (x = -13.0, y = 0.0, z = 0.0, vx = 0.0, 0, 0, 1.0, 0); color = 0x4aa8ff }

        # --- Module 2: Kepler orbit about a static sun, centre -4 ---
        entity sun    { state = (-4.0, 0.0, 0.0, 0, 0, 0, 2.6, 0); color = 0xffb347 }
        entity planet { state = (x = 0.0, y = 0.0, z = 0.0, vx = 0.0, vy = 1.15, vz = 0.0, 1.6, 0); color = 0x8fe3e8 }

        # --- Module 3: reversible kinetics A <-> B, centre +4 ---
        entity chem { state = (4.0, 0.0, 0.0, a = 1.0, b = 0.0, 0, 0, 0); color = 0x11141c }
        entity chemA { state = (3.0, 2.0, 0.0, 0, 0, 0, 1.0, 0); color = 0xff6b4a }
        entity chemB { state = (5.0, 2.0, 0.0, 0, 0, 0, 0.3, 0); color = 0x6bff8a }

        # --- Module 4: logistic growth (N' = rN(1-N/K)), centre +12 ---
        entity pop { state = (12.0, 0.0, 0.0, 0, 0, 0, 0.3, 0); color = 0xe8c39e }

        # --- Module 5: radioactive decay (N' = -l N), centre +16 ---
        entity iso { state = (16.0, 0.0, 0.0, 0, 0, 0, 2.0, 0); color = 0xd6ff8a }

        # --- Module 6: bang-bang thermostat (branch control), centre +20 ---
        # Heats below 18 °C, cools above 20 °C (hysteresis); `alarm` glows
        # while the heater is on. Uses `and` / `or` / `not` + `if`.
        entity thermo { state = (temp = 10.0, h = 1.0, 0, 0, 0, 0, 0.35, 0); color = 0x9d4edd }
        entity alarm { state = (20.0, 2.5, 0.0, 0, 0, 0, 0.06, 0); color = 0xff4d6d }
    }
    systems {
        # 1. Harmonic oscillator: x'' = -4 (x + 12), i.e. centre at -12.
        rk4 { on = hosc; dt = 0.002
            x  = vx
            vx = -4.0 * (x + 12.0) }
        # 2. Kepler: planet accelerates toward the sun (-4, 0) as -GM r / r^3.
        rk4 { on = planet; dt = 0.002
            let rx = x + 4.0
            let r  = hypot(rx, y)
            let r3 = r * r * r
            x  = vx
            vx = -4.0 * rx / r3
            y  = vy
            vy = -4.0 * y / r3 }
        # 3. Reversible kinetics A <-> B, mass conserved (a + b = 1).
        rk4 { on = chem; dt = 0.002
            a = -0.5 * a + 0.4 * b
            b =  0.5 * a - 0.4 * b }
        update { on = chemA; dt = 0.002
            s6 = 300.0 * (@chem.a - s6) }
        update { on = chemB; dt = 0.002
            s6 = 300.0 * (@chem.b - s6) }
        # 4. Logistic growth to carrying capacity K = 2.
        rk4 { on = pop; dt = 0.002
            s6 = 0.8 * s6 * (1.0 - s6 / 2.0) }
        # 5. Radioactive decay N' = -0.15 N.
        rk4 { on = iso; dt = 0.002
            s6 = -0.15 * s6 }
        # 6. Thermostat: hysteresis band 18..20, heating lifts toward 36.
        # `h` flips instantly (dt*10 compensates dt) so the band is a true
        # Schmitt trigger; a lagged h would freeze mid-band and limit-cycle.
        update { on = thermo; dt = 0.1
            let newh = if(temp < 18.0, 1.0, if(temp > 20.0, 0.0, h))
            let heating = newh and not (temp > 30.0)
            let window = (temp < 15.0) or (temp > 25.0)
            temp = heating * 1.2 - (temp - 16.0) * 0.06
            h = (newh - h) * 10.0
        }
        update { on = alarm; dt = 0.1
            s6 = 300.0 * (@thermo.h - s6) }
    }
"#;

fn s(rt: &LangRuntime, id: u128, slot: usize) -> f64 {
    rt.scene
        .get(EntityId(id))
        .and_then(|e| e.state.as_ref())
        .unwrap()
        .values[slot]
}

fn main() -> pwe_api::Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);
    let mut rt = LangRuntime::compile(SOURCE)?;

    let live = Arc::new(RwLock::new(LiveState::default()));
    present::serve_live(
        Arc::clone(&live),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        port,
    )
    .expect("serve live viewer");
    println!("live Scientific Laws gallery: open http://localhost:{port}");

    let cam = CameraVisual {
        position: pwe_reference::math::Vec3::new(2.0, 5.0, 30.0),
        target: pwe_reference::math::Vec3::new(2.0, 0.0, 0.0),
    };

    let mut step = 0u64;
    loop {
        rt.step_cross()?; // interpreter == JIT, every step
        step += 1;
        let mut g = live.write().unwrap();
        g.step = step;
        g.frame = rt.present_frame(Some(cam));
        // Live law values from each module.
        let hx = s(&rt, 1, 0); // harmonic x
        let (px, py) = (s(&rt, 3, 0), s(&rt, 3, 1)); // planet x,y
        let (a, b) = (s(&rt, 4, 3), s(&rt, 4, 4)); // A, B
        let n = s(&rt, 7, 6); // logistic pop size
        let iso = s(&rt, 8, 6); // isotope remaining
        let (temp, heat) = (s(&rt, 9, 0), s(&rt, 9, 1)); // thermostat temp, heater
        g.info = vec![
            format!("step {step}: scientific laws gallery (cross-backend)"),
            format!(
                "harmonic x={hx:6.2} | kepler planet=({px:5.2},{py:5.2})"
            ),
            format!(
                "chem A={a:5.3} B={b:5.3} (A+B={:.3}) | logistic N={n:5.3} (→2) | decay N={iso:5.3}",
                a + b
            ),
            format!("thermostat temp={temp:5.2} heater={heat:.0} (bang-bang 18–20 °C)"),
        ];
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}
