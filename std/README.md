# `std/` — the PWE standard library

A foundational package of PWE modules (`import "std/<name>"`) covering the
domains a general simulation language needs: mathematics, point kinematics,
forces, rigid-body mechanics, chemistry, thermal physics, acoustics, optics,
electromagnetism, and robotics.

## How modules are used

```pwe
import "std/forces"
import "std/thermal"

world {
  gravity = (0, 0, 0)
  entity body { state = (x = 1.0, vx = 0.0, temp = 353.15) }
}
systems {
  update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    x    = vx + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
  }
}
```

Every module is plain PWE: it declares **pure scalar functions** and physical
**parameters** (namespaced, e.g. `chemistry.R_gas`). A program imports the
modules whose laws it needs; parameters merge into the model and are
overridable with `pwe run --param <module>.<name>=…`. Functions are called
through the module's namespace (`math.lerp(...)`).

A composing example lives at `cli/examples/domains.pwe`.

## Modules

| module | contents |
| --- | --- |
| `std/math` | `clamp clamp01 lerp mix inv_lerp remap step smoothstep smootherstep wrap sqr deg rad hypot2 hypot3 min3 max3 sgn deadzone ease_in ease_out` |
| `std/particles` | `terminal_velocity drag_step ballistic_x ballistic_y ballistic_vy bounce_vy reflect radius_from_mass stopping_distance freefall_time speed` |
| `std/forces` | `hooke spring_accel damping_accel drag_linear_accel drag_quadratic_accel coulomb_force gravity_force inverse_square_accel buoyancy_force thrust_accel damper_force` |
| `std/mechanics` | `momentum kinetic_energy reduced_mass elastic_1d_v1 elastic_1d_v2 impulse friction_force normal_impulse inertia_rod inertia_disk inertia_sphere torque angular_accel angular_kinetic` |
| `std/chemistry` | Periodic table 1–118: `atomic_mass element_period element_group valence_electrons shell_capacity neutrons`, plus `mol_from_mass mass_from_mol molecules molarity dilute ideal_pressure ideal_volume arrhenius rate_constant activation_ratio ph h_from_ph neutralization_volume half_life_decay radioactive_amount` (params `R_gas`, `avogadro`) |
| `std/thermal` | `celsius kelvin newton_cooling heat_capacity sensible_heat conduction_flux stefan_boltzmann radiative_cooling thermal_diffusivity thermostat_hysteresis mixing_temp` (param `sigma_sb`) |
| `std/acoustics` | `speed_of_sound_air wavelength spl pressure_from_spl acoustic_impedance doppler inverse_square sound_intensity beat_frequency` (params `rho_air`, `c_air`, `p_ref`) |
| `std/optics` | `inverse_square_intensity beer_lambert snell_angle critical_angle fresnel_reflectance reflect_axis wien_peak photon_energy photon_energy_wavelength focal_length` (params `h_planck`, `c_light`) |
| `std/em` | `coulomb_force electric_field potential lorentz_force cyclotron_radius biot_savart_wire poynting plane_wave_b plane_wave_e impedance_free_space` (params `c_light`, `k_coulomb`, `mu0`) |
| `std/robotics` | `planar2_x planar2_y planar2_ik_q1 planar2_ik_q2 pid joint_accel diff_drive_v_left diff_drive_v_right trapezoid_peak reach rotate_x rotate_y` |
| `std/units` | `kmh_to_ms ms_to_kmh mph_to_ms knot_to_ms fahrenheit_to_celsius celsius_to_fahrenheit celsius_to_kelvin kelvin_to_celsius kwh_to_j cal_to_j kcal_to_j ev_to_j btu_to_j atm_to_pa bar_to_pa psi_to_pa mmhg_to_pa angstrom_to_m nm_to_m au_to_m ly_to_m parsec_to_m mile_to_m nautical_mile_to_m foot_to_m inch_to_m lb_to_kg oz_to_kg tonne_to_kg deg_to_rad rad_to_deg arcmin_to_rad arcsec_to_rad g_to_ms2 lbf_to_n` |
| `std/control` | `first_order second_order low_pass complementary integrate derivative pid pid_clamped feedforward state_feedback bang_bang hysteresis rate_limit_delta lead within slew` |

## Field solver system kinds

Continuum domains step a grid field with a solver declaration instead of an
explicit loop:

- `diffuse { field = <name>; rate = r }` — explicit diffusion, `T += r·∇²T`
  per step. Implemented as a **Jacobi** sweep (two passes over the cells), so
  with the zero-flux stencil the total is exactly conserved. Stability:
  `rate ≤ 1/4` in 2D, `≤ 1/6` in 3D.
- `poisson { field = <name>; source = <name>?; iters = n; scale = s }` —
  Gauss–Seidel relaxation of `∇²φ = ρ·scale`; boundary cells are held fixed
  (fixed potentials).
- `wave { field = <name>; prev = <name>; velocity = c; dt = h }` — second-order
  leapfrog `u_tt = c²∇²u` over two fields (`prev` holds `u(t−h)`); the initial
  pulse splits into a spherical wavefront. Stability `c·h/dx ≤ 1/√2` (2D), `≤ 1/√3` (3D); `absorb`/`absorb_width` add a graded sponge layer that soaks up outgoing waves (open boundary), and an optional `damping` scales the temporal term.

Both run once per step (only the first dynamic entity emits the sweep), lower
to the existing `fget`/`fset`/`flap` EIR — so the interpreter and JIT stay
byte-identical — and are covered by cross-backend tests.

## Conventions

- Functions are **pure and scalar**; vectors are passed/returned as separate
  components (`planar2_x(q1, q2)`, `planar2_y(q1, q2)`) because PWE expressions
  are scalar.
- Physical constants are module **parameters** so they can be overridden
  without editing the library. Builtins `pi`/`e` are available directly.
- Units are annotated on parameters; functions are unit-agnostic.

## Non-goals

- Causal/networked multi-world concerns (handled by `send`/`recv`, channels).
- Target backends (GPU/NPU/SIMD — see `tasks/todo.md`).
- Anything requiring dynamic entity creation (the entity set is fixed at
  compile time until object-pool activation lands).
