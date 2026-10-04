use pwe_api::EntityId;
use pwe_reference::lang::LangRuntime;
use pwe_reference::present::Shape;

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
    "des",
    "signal",
    "md",
    "periodic",
    "micro",
    "atoms/O",
    "atoms/Fe",
    "elements/Fe",
    "elements/F",
    "elements/He",
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

/// RFC-0047 step 5 / RFC-0048: the sampled-data (signal/DSP) library — decibel
/// conversions, first-order filters, trapezoidal integration, RBJ biquad
/// coefficients, envelope/measurement helpers, and tone/pitch conversion. All
/// pure, trap-free, and evaluated cross-backend.
#[test]
fn signal_module_computes_dsp_primitives() {
    let v = eval(
        "signal",
        &[
            ("db0", "signal.db(1.0)"),
            ("dbm", "signal.db(0.001)"),
            ("fdb", "signal.from_db(20.0)"),
            ("pdb", "signal.power_db(100.0)"),
            ("a_tau", "signal.alpha_from_tau(1.0, 1.0)"),
            ("a_fc", "signal.alpha_from_fc(0.0, 1.0)"),
            ("onepole", "signal.one_pole(0.0, 10.0, 0.5)"),
            ("trapr", "signal.integrate_trap(0.0, 2.0, 0.0, 1.0)"),
            ("ders", "signal.safe_div(6.0, -2.0)"),
            ("quant", "signal.quantize(2.3, 1.0)"),
            (
                "bq_b0",
                "signal.lowpass_b0(1000.0, 0.7071067811865476, 48000.0)",
            ),
            ("midi", "signal.midi_to_hz(69.0)"),
            ("midi2", "signal.hz_to_midi(880.0)"),
            ("wrap", "signal.osc_sin(0.25)"),
            ("clip", "signal.soft_clip(0.5, 3.0)"),
            ("env", "signal.envelope(0.0, 1.0, 0.5, 0.1)"),
        ],
    );
    assert!(close(v[0], 0.0), "db(1) = {}", v[0]);
    assert!(close(v[1], -60.0), "db(0.001) = {}", v[1]);
    assert!(close(v[2], 10.0), "from_db(20) = {}", v[2]);
    assert!(close(v[3], 20.0), "power_db(100) = {}", v[3]);
    assert!(close(v[4], 0.5), "alpha_from_tau(1,1) = {}", v[4]);
    assert!(close(v[5], 1.0), "alpha_from_fc(0,1) = {}", v[5]);
    assert!(close(v[6], 5.0), "one_pole = {}", v[6]);
    assert!(close(v[7], 1.0), "integrate_trap = {}", v[7]);
    assert!(close(v[8], -3.0), "safe_div(6,-2) = {}", v[8]);
    assert!(close(v[9], 2.0), "quantize(2.3,1) = {}", v[9]);
    assert!(close(v[10], 0.003916), "biquad b0 = {}", v[10]);
    assert!(close(v[11], 440.0), "midi_to_hz(69) = {}", v[11]);
    assert!(close(v[12], 81.0), "hz_to_midi(880) = {}", v[12]);
    assert!(close(v[13], 1.0), "osc_sin(0.25) = {}", v[13]);
    assert!(close(v[14], 0.909646), "soft_clip(0.5,3) = {}", v[14]);
    assert!(close(v[15], 0.5), "envelope (attack) = {}", v[15]);
}

/// RFC-0047 step 5: the biquad *difference equation* is a stable one-step
/// recurrence — a low-pass driven by a DC input settles to the DC gain, proving
/// the coefficient set and `biquad` agree. Cross-backend.
#[test]
fn signal_biquad_settles_to_dc_gain() {
    // Canonical direct-form-I low-pass driven by a unit DC step. Every next
    // value is computed in a `let` (reading old state) before any slot is
    // written, so the result is independent of rule-ordering: the output
    // settles to the filter's DC gain, which is 1 for an RBJ low-pass.
    let src = format!(
        r#"
        import "{}/signal"
        world {{ gravity = (0, 0, 0)
            entity f {{ state = (y = 0.0, x1 = 0.0, x2 = 0.0, y1 = 0.0, y2 = 0.0) }} }}
        systems {{ update {{ on = f; dt = 0.001
            let b0 = signal.lowpass_b0(100.0, 0.7071067811865476, 1000.0)
            let b1 = signal.lowpass_b1(100.0, 0.7071067811865476, 1000.0)
            let b2 = signal.lowpass_b2(100.0, 0.7071067811865476, 1000.0)
            let a1 = signal.lowpass_a1(100.0, 0.7071067811865476, 1000.0)
            let a2 = signal.lowpass_a2(100.0, 0.7071067811865476, 1000.0)
            let xn = 1.0
            let yn = signal.biquad(b0, b1, b2, a1, a2, xn, x1, x2, y1, y2)
            let x1n = xn
            let x2n = x1
            let y1n = yn
            let y2n = y1
            x1 = x1n
            x2 = x2n
            y = yn
            y1 = y1n
            y2 = y2n }}
        }}
    "#,
        std_dir()
    );
    let path = std::env::temp_dir().join("pwe_stdlib_signal_biquad.pwe");
    std::fs::write(&path, src).unwrap();
    let mut rt = LangRuntime::compile_file(&path).unwrap();
    for _ in 0..256 {
        rt.step_cross().unwrap();
    }
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        close(st.values[0], 1.0),
        "biquad low-pass DC gain = {}, want 1.0",
        st.values[0]
    );
    assert!(st.values.iter().all(|v| v.is_finite()), "stable: {st:?}");
}

/// RFC-0047 step 6: the molecular-dynamics library — periodic boundary
/// conditions (minimum image / wrapping), lattice + density helpers, kinetic
/// temperature, and Berendsen thermostat/barostat scale factors. Pure,
/// trap-free, evaluated cross-backend.
#[test]
fn md_module_computes_pbc_and_thermostat() {
    let v = eval(
        "md",
        &[
            ("wrap", "md.wrap(0.0 - 1.0, 10.0)"),
            ("wrap_s", "md.wrap_signed(7.0, 10.0)"),
            ("mi", "md.min_image(7.0, 10.0)"),
            ("mid", "md.min_image_dist(11.0, 0.0, 0.0, 10.0)"),
            ("mid2", "md.min_image_dist2(11.0, 0.0, 0.0, 10.0)"),
            ("rho", "md.number_density(100.0, 10.0)"),
            ("lfcc", "md.lattice_fcc(0.1)"),
            ("nnbcc", "md.nn_dist_bcc(0.1)"),
            ("dof", "md.degrees_of_freedom(2.0, 1.0)"),
            ("temp", "md.temperature_from_velocity(3.0e-23, 2.0, 1.0)"),
            ("bke", "md.kinetic_energy(300.0, 2.0, 1.0)"),
            ("blam", "md.berendsen_lambda(100.0, 120.0, 0.001, 0.1)"),
            ("half", "md.half_kick(1.0, 10.0, 2.0, 0.5)"),
            ("drift", "md.drift(0.0, 2.0, 0.5)"),
            ("msd", "md.msd(1.0, 2.0, 2.0)"),
            ("diff", "md.diffusion(6.0, 1.0, 3.0)"),
        ],
    );
    assert!(close(v[0], 9.0), "wrap(-1,10) = {}", v[0]);
    assert!(close(v[1], -3.0), "wrap_signed(7,10) = {}", v[1]);
    assert!(close(v[2], -3.0), "min_image(7,10) = {}", v[2]);
    assert!(close(v[3], 1.0), "min_image_dist(11,0,0,10) = {}", v[3]);
    assert!(close(v[4], 1.0), "min_image_dist2 = {}", v[4]);
    assert!(close(v[5], 0.1), "number_density(100,10) = {}", v[5]);
    assert!(close(v[6], 3.419952), "lattice_fcc(0.1) = {}", v[6]);
    assert!(close(v[7], 2.350755), "nn_dist_bcc(0.1) = {}", v[7]);
    assert!(close(v[8], 3.0), "dof(2, remove_com) = {}", v[8]);
    assert!(
        close(v[9], 0.724297),
        "temperature_from_velocity = {}",
        v[9]
    );
    assert!(
        close(v[10], 6.2129205e-21),
        "kinetic_energy(300) = {}",
        v[10]
    );
    assert!(close(v[11], 1.001), "berendsen_lambda = {}", v[11]);
    assert!(close(v[12], 2.25), "half_kick = {}", v[12]);
    assert!(close(v[13], 1.0), "drift = {}", v[13]);
    assert!(close(v[14], 9.0), "msd = {}", v[14]);
    assert!(close(v[15], 1.0), "diffusion = {}", v[15]);
}

/// RFC: the full periodic table (`std/periodic.pwe` + one module per element)
/// provides accurate values and per-element accessors.
#[test]
fn periodic_table_lookups() {
    let count = std::fs::read_dir(format!("{}/elements", std_dir()))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "pwe"))
        .count();
    assert_eq!(count, 118, "one `std/elements/<Symbol>.pwe` per element");

    let v = eval(
        "periodic",
        &[
            ("fe_mass", "periodic.atomic_mass(26.0)"),
            ("f_en", "periodic.electronegativity(9.0)"),
            ("he_noble", "periodic.is_noble_gas(2.0)"),
            ("o_val", "periodic.valence_electrons(8.0)"),
            ("fe_group", "periodic.group(26.0)"),
            ("fe_block", "periodic.block(26.0)"),
            ("u_period", "periodic.period(92.0)"),
            ("fe_mass_elem", "Fe.atomic_mass()"),
            ("f_en_elem", "F.electronegativity()"),
            ("he_gas_elem", "He.is_gas_at_stp()"),
            ("o_electrons", "periodic.electrons(8.0)"),
            ("o_neutrons", "periodic.neutrons(8.0)"),
            ("o_shells", "periodic.shell_count(8.0)"),
            ("o_shell1", "periodic.shell_electrons(8.0, 1.0)"),
            ("o_shell2", "periodic.shell_electrons(8.0, 2.0)"),
            ("fe_shell3", "Fe.shell_electrons(3.0)"),
        ],
    );
    assert!(close(v[0], 55.8452), "Fe mass = {}", v[0]);
    assert!(close(v[1], 3.98), "F electronegativity = {}", v[1]);
    assert!(close(v[2], 1.0), "He is noble = {}", v[2]);
    assert!(close(v[3], 6.0), "O valence = {}", v[3]);
    assert!(close(v[4], 8.0), "Fe group = {}", v[4]);
    assert!(close(v[5], 3.0), "Fe is a d-block element = {}", v[5]);
    assert!(close(v[6], 7.0), "U period = {}", v[6]);
    assert!(close(v[7], 55.8452), "Fe module mass = {}", v[7]);
    assert!(close(v[8], 3.98), "F module electronegativity = {}", v[8]);
    assert!(close(v[9], 1.0), "He module is a gas = {}", v[9]);
    assert!(close(v[10], 8.0), "O electrons = {}", v[10]);
    assert!(close(v[11], 8.0), "O neutrons = {}", v[11]);
    assert!(close(v[12], 2.0), "O shells = {}", v[12]);
    assert!(close(v[13], 2.0), "O shell 1 = {}", v[13]);
    assert!(close(v[14], 6.0), "O shell 2 = {}", v[14]);
    assert!(close(v[15], 14.0), "Fe shell 3 = {}", v[15]);
}

/// A molecule module declares renderable atoms + `bond` lines and composes its
/// molar mass from the element modules it imports.
#[test]
fn molecule_modules_build_atoms_and_bonds() {
    let path = format!("{}/molecules/water.pwe", std_dir());
    let rt = LangRuntime::compile_file(std::path::Path::new(&path)).unwrap();
    let frame = rt.present_frame(None);
    assert_eq!(frame.bonds.len(), 2, "water has two O-H bonds");
    assert_eq!(frame.entities.len(), 3, "water has three atoms");

    // `water.molar_mass()` / `atom_count()` evaluate correctly when imported.
    let src = format!(
        "import \"{}/molecules/water\"\nworld {{\n  gravity = (0, 0, 0)\n           entity probe {{ state = (mm = 0.0, n = 0.0) }}\n}}\n         systems {{\n  update {{ on = probe; dt = 1.0\n    mm = water.molar_mass() + 0.0\n             n = water.atom_count() + 0.0\n  }}\n}}\n",
        std_dir()
    );
    let p = std::env::temp_dir().join("pwe_molecule_water.pwe");
    std::fs::write(&p, &src).unwrap();
    let mut rt = LangRuntime::compile_file(&p).unwrap();
    rt.step_cross_n(1).unwrap();
    let st = rt.scene.get(EntityId(1)).unwrap().state.as_ref().unwrap();
    assert!(
        close(st.values[0], 18.015),
        "water molar mass = {}",
        st.values[0]
    );
    assert!(
        close(st.values[1], 3.0),
        "water atom count = {}",
        st.values[1]
    );
}

/// Layer A: `std/atoms/<Sym>` declares a schematic atomic structure — a nucleus
/// (protons + neutrons) and orbiting electrons on shell rings.
#[test]
fn atom_modules_render_nucleus_and_orbiting_electrons() {
    let src = format!(
        "import \"{}/atoms/O\"\nworld {{\n  gravity = (0, 0, 0)\n  \
         entity a {{ shape = O_atom; size = 2.0 }}\n}}\n",
        std_dir()
    );
    let p = std::env::temp_dir().join("pwe_atom_o.pwe");
    std::fs::write(&p, &src).unwrap();
    let rt = LangRuntime::compile_file(&p).unwrap();
    let frame = rt.present_frame(None);
    match &frame.entities[0].shape {
        Shape::Group(parts) => {
            let rings = parts
                .iter()
                .filter(|p| matches!(p.shape, Shape::Ring { .. }))
                .count();
            let orbiting = parts.iter().filter(|p| p.orbit.is_some()).count();
            assert_eq!(rings, 2, "oxygen has two occupied shells");
            assert!(orbiting >= 2, "electrons orbit: {orbiting}");
            assert!(
                parts.iter().any(|p| p.color == Some(0xFF_5555)),
                "protons are coloured"
            );
        }
        other => panic!("expected a composite shape, got {other:?}"),
    }
}
