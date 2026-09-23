use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;

fn std_dir() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../std")
}

const MODULES: &[&str] = &[
    "math",
    "particles",
    "forces",
    "mechanics",
    "chemistry",
    "thermal",
    "acoustics",
    "optics",
    "em",
    "robotics",
    "units",
    "control",
];

/// Every standard-library module is a valid standalone program.
#[test]
fn all_std_modules_compile() {
    for m in MODULES {
        let path = format!("{}/{m}.pwe", std_dir());
        let mut rt = LangRuntime::compile_file(std::path::Path::new(&path))
            .unwrap_or_else(|e| panic!("std/{m}.pwe failed to compile: {e:?}"));
        rt.step_cross_n(1)
            .unwrap_or_else(|e| panic!("std/{m}.pwe failed to step: {e:?}"));
    }
}

/// Compiles a model importing every std module, evaluates each `(slot, expr)`
/// rule for one step, and returns the slot values in order. Kept within the
/// 16-state-slot cap (callers pass at most 16 rules).
fn eval(tag: &str, rules: &[(&str, &str)]) -> Vec<f64> {
    assert!(rules.len() <= 16, "state-slot cap");
    let imports = MODULES
        .iter()
        .map(|m| format!("import \"{}/{m}\"\n", std_dir()))
        .collect::<String>();
    let slots = rules
        .iter()
        .map(|(name, _)| format!("{name} = 0.0"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = rules
        .iter()
        .map(|(name, expr)| format!("{name} = {expr} + 0.0"))
        .collect::<Vec<_>>()
        .join("\n    ");
    let src = format!(
        "{imports}\nworld {{\n  gravity = (0, 0, 0)\n  entity e {{ state = ({slots}) }}\n}}\n\
         systems {{\n  update {{ on = e; dt = 1.0\n    {body}\n  }}\n}}\n"
    );
    let path = std::env::temp_dir().join(format!("pwe_stdlib_{tag}.pwe"));
    std::fs::write(&path, &src).unwrap();
    let mut rt = LangRuntime::compile_file(&path).unwrap();
    rt.step_cross_n(1).unwrap();
    rt.scene
        .get(EntityId(1))
        .unwrap()
        .state
        .as_ref()
        .unwrap()
        .values[..rules.len()]
        .to_vec()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// Cross-module calls evaluate to the expected physics (batch 1).
#[test]
fn std_modules_compute_physics() {
    let v = eval(
        "physics",
        &[
            ("lerp", "math.lerp(0.0, 10.0, 0.25)"),
            ("smooth", "math.smoothstep(0.0, 1.0, 0.5)"),
            ("wrapv", "math.wrap(7.5, 0.0, 1.0)"),
            ("ball", "particles.ballistic_y(0.0, 10.0, 9.8, 1.0)"),
            ("spr", "forces.spring_accel(2.0, 3.0, 1.0)"),
            ("ela", "mechanics.elastic_1d_v1(1.0, 2.0, 1.0, 0.0)"),
            ("cel", "thermal.celsius(300.0)"),
            ("splv", "acoustics.spl(0.02)"),
            ("wien", "optics.wien_peak(300.0)"),
            ("pot", "em.potential(1.0, 1.0)"),
            ("fk", "robotics.planar2_x(1.0, 1.0, 0.0, 0.0)"),
            ("pid0", "robotics.pid(2.0, 1.0, 0.5, 1.0, 0.0, 0.0)"),
        ],
    );
    assert!(close(v[0], 2.5), "lerp = {}", v[0]);
    assert!(close(v[1], 0.5), "smoothstep = {}", v[1]);
    assert!(close(v[2], 0.5), "wrap = {}", v[2]);
    assert!(close(v[3], 5.1), "ballistic_y = {}", v[3]);
    assert!(close(v[4], -6.0), "spring_accel = {}", v[4]);
    assert!(close(v[5], -1.0 / 3.0), "elastic_1d_v1 = {}", v[5]);
    assert!(close(v[6], 26.85), "celsius = {}", v[6]);
    assert!(close(v[7], 60.0), "spl = {}", v[7]);
    assert!(close(v[8], 2.897771955e-3 / 300.0), "wien_peak = {}", v[8]);
    assert!(close(v[9], 8.9875517923e9), "potential = {}", v[9]);
    assert!(close(v[10], 2.0), "planar2_x = {}", v[10]);
    assert!(close(v[11], 2.0), "pid = {}", v[11]);
}

/// Element data (full 1–118 table), unit conversions, and control primitives
/// (batch 2).
#[test]
fn std_modules_compute_data() {
    let v = eval(
        "data",
        &[
            ("amass", "chemistry.atomic_mass(6.0)"),
            ("umass", "chemistry.atomic_mass(92.0)"),
            ("uperiod", "chemistry.element_period(92.0)"),
            ("ugroup", "chemistry.element_group(26.0)"),
            ("ulanth", "chemistry.element_group(57.0)"),
            ("uval", "chemistry.valence_electrons(6.0)"),
            ("kmh", "units.kmh_to_ms(36.0)"),
            ("psi", "units.psi_to_pa(1.0)"),
            ("f2c", "units.fahrenheit_to_celsius(212.0)"),
            ("lag", "control.first_order(0.0, 10.0, 2.0)"),
            (
                "clip",
                "control.pid_clamped(1.0, 0.0, 0.0, 5.0, 0.0, 0.0, -1.0, 1.0)",
            ),
            ("band", "control.within(0.1, 0.5)"),
        ],
    );
    assert!(close(v[0], 12.011), "atomic_mass(6) = {}", v[0]);
    assert!(close(v[1], 238.02891), "atomic_mass(92) = {}", v[1]);
    assert!(close(v[2], 7.0), "element_period(92) = {}", v[2]);
    assert!(close(v[3], 8.0), "element_group(26) = {}", v[3]);
    assert!(close(v[4], 3.0), "element_group(57, lanthanide) = {}", v[4]);
    assert!(close(v[5], 4.0), "valence_electrons(6) = {}", v[5]);
    assert!(close(v[6], 10.0), "kmh_to_ms(36) = {}", v[6]);
    assert!(close(v[7], 6894.757293168), "psi_to_pa(1) = {}", v[7]);
    assert!(close(v[8], 100.0), "fahrenheit_to_celsius(212) = {}", v[8]);
    assert!(close(v[9], 5.0), "first_order = {}", v[9]);
    assert!(close(v[10], 1.0), "pid_clamped = {}", v[10]);
    assert!(close(v[11], 1.0), "within = {}", v[11]);
}
