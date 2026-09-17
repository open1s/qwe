//! Generic continuum runtime demo: one domain-neutral scalar `Field` simulates
//! two different physical systems.
//!
//!  1. Electromagnetics: the electrostatic potential in a grounded slab under a
//!     charge density, solved by Gauss–Seidel Poisson relaxation and compared
//!     to the analytic parabola  φ(x) = ρ·x·(L−x)/(2ε₀).
//!  2. Chemistry: Fickian diffusion of a concentration spike, which conserves
//!     total amount and spreads the peak.
//!
//! Run: `cargo run --example continuum_demo`

use pwe_reference::continuum::{diffuse, electrostatic_slab};

fn main() {
    // --- Electromagnetics: grounded parallel plates ---------------------------
    let width = 65;
    let dx = 0.1;
    let length = (width as f64 - 1.0) * dx;
    let rho = 1.0;
    let eps0 = 1.0;
    let phi = electrostatic_slab(width, dx, rho, eps0, 40_000);

    let mut max_err = 0.0f64;
    let mut midpoint = 0.0;
    for i in 0..width {
        let x = i as f64 * dx;
        let analytic = (rho / (2.0 * eps0)) * x * (length - x);
        let err = (phi.value(i, 0) - analytic).abs();
        max_err = max_err.max(err);
        if i == width / 2 {
            midpoint = phi.value(i, 0);
        }
    }
    println!("== electromagnetics: electrostatic potential in a grounded slab ==");
    println!("  potential at midpoint   = {midpoint:.4} V");
    println!(
        "  analytic  at midpoint   = {:.4} V",
        (rho / (2.0 * eps0)) * (length / 2.0) * (length / 2.0)
    );
    println!("  max |numeric − analytic| = {max_err:.3e}");
    println!("  relaxation hashes        = {:?}", phi.hash());

    // --- Chemistry: diffusion conserves mass ----------------------------------
    use pwe_reference::field::Field;
    let mut c = Field::new(40, 1, 0.1);
    c.set(20, 0, 10.0);
    let total_before = c.total();
    let dt = 1e-3;
    for _ in 0..4000 {
        diffuse(&mut c, dt, 0.5);
    }
    let total_after = c.total();
    let peak = c.cells().iter().cloned().fold(0.0f64, f64::max);
    println!("\n== chemistry: Fickian diffusion of a concentration spike ==");
    println!("  total amount before = {total_before:.6}");
    println!("  total amount after  = {total_after:.6}");
    println!(
        "  mass drift          = {:.3e}",
        (total_after - total_before).abs()
    );
    println!("  peak after diffusion = {peak:.4} (was 10.0, spreads out)");
    println!("  field hash           = {:?}", c.hash());

    // --- Same field, different physics: determinism + content identity --------
    println!("\n== the generic field is content-addressed and deterministic ==");
    let a = electrostatic_slab(width, dx, rho, eps0, 40_000);
    let b = electrostatic_slab(width, dx, rho, eps0, 40_000);
    println!("  identical runs hash equal: {}", a.hash() == b.hash());
}
