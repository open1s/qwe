//! Continuum domain systems built on the generic [`crate::field::Field`].
//!
//! This shows the runtime is *generic*: the same domain-neutral field mechanism
//! simulates two very different physical systems —
//!
//! * **Electromagnetics** (`electrostatic_slab`): the electrostatic potential in
//!   a grounded slab under a charge density, found by Gauss–Seidel relaxation of
//!   the Poisson equation `∇²φ = −ρ/ε₀`.
//! * **Chemistry** (`diffuse`): Fickian reaction–diffusion of a concentration,
//!   which conserves total amount — a mass-conservation invariant.
//!
//! Both are deterministic and content-hashable, so they snapshots/replay like
//! any other world state. The domain policy lives here, not in the runtime core.

use crate::field::Field;

/// Returns the equilibrium potential in a `width`-cell slab of cell size `dx`,
/// with grounded plates (`φ = 0`) at both ends, constant charge density `rho`,
/// and permittivity `eps0`.
///
/// Analytic solution (1D Poisson, grounded ends):
/// `φ(x) = (ρ / (2 ε₀)) · x · (L − x)`.
pub fn electrostatic_slab(width: usize, dx: f64, rho: f64, eps0: f64, iterations: usize) -> Field {
    // 1D field stored as `width × 1`.
    let mut phi = Field::new(width, 1, dx);
    let source = vec![rho; width];
    let mut fixed = vec![false; width];
    // Dirichlet boundary: grounded plates at both ends.
    fixed[0] = true;
    fixed[width - 1] = true;
    let scale = -1.0 / eps0;
    for _ in 0..iterations {
        phi.poisson_step(&source, scale, &fixed);
    }
    phi
}

/// Runs one explicit diffusion step of a concentration `c` with diffusivity `D`
/// and step `dt`. The field's total amount is preserved (mass conservation).
pub fn diffuse(c: &mut Field, dt: f64, diffusivity: f64) {
    c.diffusion_step(dt, diffusivity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    #[test]
    fn electrostatic_slab_matches_analytic_solution() {
        // Grounded parallel plates, uniform charge density. The relaxed numeric
        // potential must match the analytic parabola.
        let width = 64;
        let dx = 0.1;
        let length = (width as f64 - 1.0) * dx; // L
        let rho = 1.0;
        let eps0 = 1.0;
        let phi = electrostatic_slab(width, dx, rho, eps0, 20_000);

        let mut max_err = 0.0f64;
        for i in 0..width {
            let x = i as f64 * dx;
            let analytic = (rho / (2.0 * eps0)) * x * (length - x);
            let err = (phi.value(i, 0) - analytic).abs();
            max_err = max_err.max(err);
        }
        // Converged to the analytic quadratic within numeric tolerance.
        assert!(
            max_err < 1e-2,
            "electrostatic potential diverged from analytic, max_err={max_err}"
        );
    }

    #[test]
    fn diffusion_conserves_total_amount() {
        // A concentration spike spreads by Fickian diffusion; total amount
        // (the integral of c) is conserved.
        let mut c = Field::new(32, 1, 0.1);
        c.set(16, 0, 10.0);
        let before = c.total();
        let dt = 1e-3;
        for _ in 0..2000 {
            diffuse(&mut c, dt, 0.5);
        }
        let after = c.total();
        assert!(
            (before - after).abs() < 1e-9,
            "mass not conserved: before={before} after={after}"
        );
        // The spike spread out: the peak is far below the initial 10.0.
        let peak = c.cells().iter().cloned().fold(0.0f64, f64::max);
        assert!(peak < 10.0, "expected the spike to spread, peak={peak}");
        assert!(peak > 0.0, "concentration should remain positive");
    }

    #[test]
    fn fields_are_deterministic_and_replayable() {
        // Identical EM relaxation runs produce identical content hashes.
        let a = electrostatic_slab(48, 0.1, 1.0, 1.0, 8_000);
        let b = electrostatic_slab(48, 0.1, 1.0, 1.0, 8_000);
        assert_eq!(a.hash(), b.hash());
        assert!(a.hash() != Field::new(48, 1, 0.1).hash());
    }
}
