//! A generic, domain-neutral continuum field on a rectilinear grid.
//!
//! The runtime is *generic*: the same field mechanism models electromagnetic
//! potential, chemical concentration, temperature, or pressure. This module is
//! pure mechanism (storage + discrete operators + determinism hash). The domain
//! policy — the governing equation, sources, and boundary conditions — lives in
//! domain systems that operate on this field (physics/EM/chemistry are peers).
//!
//! Everything is deterministic and content-hashable, so a field can be
//! snapshot/replayed like any other world state.

use crate::sha256::digest;
use pwe_api::Hash256;

/// A deterministic scalar field over a `width × height` grid with cell size
/// `dx`. Storage is row-major; boundary cells use a zero-flux (Neumann) stencil
/// unless a domain marks them fixed.
#[derive(Clone, Debug)]
pub struct Field {
    pub width: usize,
    pub height: usize,
    pub dx: f64,
    cells: Vec<f64>,
}

impl Field {
    pub fn new(width: usize, height: usize, dx: f64) -> Self {
        Self {
            width,
            height,
            dx,
            cells: vec![0.0; width * height],
        }
    }
    pub fn cells(&self) -> &[f64] {
        &self.cells
    }
    pub fn value(&self, i: usize, j: usize) -> f64 {
        self.cells[j * self.width + i]
    }
    pub fn set(&mut self, i: usize, j: usize, value: f64) {
        self.cells[j * self.width + i] = value;
    }
    pub fn total(&self) -> f64 {
        self.cells.iter().sum()
    }

    /// The 5-point Laplacian at `(i, j)`, with a zero-flux (Neumann) boundary.
    pub fn laplacian(&self, i: usize, j: usize) -> f64 {
        let (w, h) = (self.width, self.height);
        let (i, j) = (i as isize, j as isize);
        let center = self.cells[(j * w as isize + i) as usize];
        let left = if i > 0 {
            self.cells[(j * w as isize + (i - 1)) as usize]
        } else {
            center
        };
        let right = if i + 1 < w as isize {
            self.cells[(j * w as isize + (i + 1)) as usize]
        } else {
            center
        };
        let up = if j > 0 {
            self.cells[((j - 1) * w as isize + i) as usize]
        } else {
            center
        };
        let down = if j + 1 < h as isize {
            self.cells[((j + 1) * w as isize + i) as usize]
        } else {
            center
        };
        (left + right + up + down - 4.0 * center) / (self.dx * self.dx)
    }

    /// One Gauss–Seidel relaxation pass solving the Poisson equation
    /// `∇²φ = source · scale` (`fixed` marks Dirichlet cells held constant).
    /// This is the domain-neutral operator an electromagnetics or gravitation
    /// domain uses to find a steady-state potential.
    pub fn poisson_step(&mut self, source: &[f64], scale: f64, fixed: &[bool]) {
        let (w, h) = (self.width, self.height);
        let dx2 = self.dx * self.dx;
        for j in 0..h {
            for i in 0..w {
                let idx = j * w + i;
                if fixed[idx] {
                    continue;
                }
                let left = if i > 0 {
                    self.cells[idx - 1]
                } else {
                    self.cells[idx]
                };
                let right = if i + 1 < w {
                    self.cells[idx + 1]
                } else {
                    self.cells[idx]
                };
                let up = if j > 0 {
                    self.cells[idx - w]
                } else {
                    self.cells[idx]
                };
                let down = if j + 1 < h {
                    self.cells[idx + w]
                } else {
                    self.cells[idx]
                };
                // Gauss–Seidel: ∇²φ = source·scale  =>  φ = (Σ − source·scale·dx²)/4.
                self.cells[idx] = (left + right + up + down - source[idx] * scale * dx2) / 4.0;
            }
        }
    }

    /// Explicit forward-Euler diffusion `∂c/∂t = D·∇²c`, stable for small dt.
    /// A chemistry (reaction–diffusion) or temperature domain uses this.
    pub fn diffusion_step(&mut self, dt: f64, diffusivity: f64) {
        let lap: Vec<f64> = (0..self.cells.len())
            .map(|idx| {
                let i = idx % self.width;
                let j = idx / self.width;
                self.laplacian(i, j)
            })
            .collect();
        for (cell, &l) in self.cells.iter_mut().zip(&lap) {
            *cell += dt * diffusivity * l;
        }
    }

    /// Deterministic content hash (RFC-0025-compatible replay identity).
    pub fn hash(&self) -> Hash256 {
        let mut out = Vec::with_capacity(4 + self.cells.len() * 8);
        out.extend_from_slice(&(self.width as u64).to_le_bytes());
        out.extend_from_slice(&(self.height as u64).to_le_bytes());
        out.extend_from_slice(&self.dx.to_bits().to_le_bytes());
        for c in &self.cells {
            out.extend_from_slice(&c.to_bits().to_le_bytes());
        }
        digest(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laplacian_of_linear_field_is_zero() {
        // phi(x) = x  =>  d2phi/dx2 = 0 (flat in y).
        let mut f = Field::new(8, 8, 1.0);
        for j in 0..8 {
            for i in 0..8 {
                f.set(i, j, i as f64);
            }
        }
        assert!((f.laplacian(3, 3).abs() < 1e-12));
    }

    #[test]
    fn hash_is_deterministic() {
        let mut a = Field::new(4, 4, 0.5);
        let mut b = Field::new(4, 4, 0.5);
        a.set(1, 1, 2.5);
        b.set(1, 1, 2.5);
        assert_eq!(a.hash(), b.hash());
        b.set(1, 1, 3.0);
        assert_ne!(a.hash(), b.hash());
    }
}
