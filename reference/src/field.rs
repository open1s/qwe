//! A generic, domain-neutral continuum field on a rectilinear grid.
//!
//! The runtime is *generic*: the same field mechanism models electromagnetic
//! potential, chemical concentration, temperature, or pressure. This module is
//! pure mechanism (storage + discrete operators + determinism hash). The domain
//! policy — the governing equation, sources, and boundary conditions — lives in
//! domain systems that operate on this field (physics/EM/chemistry are peers).
//!
//! Fields live in **3D space** (`width × height × depth`); with the simulation
//! clock this is the 4D substrate (3D space + time). A 2D field is simply a
//! `depth = 1` grid.
//!
//! Everything is deterministic and content-hashable, so a field can be
//! snapshot/replayed like any other world state.

use crate::sha256::digest;
use pwe_api::Hash256;

/// A deterministic scalar field over a `width × height × depth` grid with cell
/// size `dx`. Storage is row-major (`k·w·h + j·w + i`); boundary cells use a
/// zero-flux (Neumann) stencil unless a domain marks them fixed.
#[derive(Clone, Debug)]
pub struct Field {
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub dx: f64,
    cells: Vec<f64>,
}

impl Field {
    /// A 2D (`depth = 1`) field.
    pub fn new(width: usize, height: usize, dx: f64) -> Self {
        Self::new3(width, height, 1, dx)
    }

    /// A 3D field (`width × height × depth`).
    pub fn new3(width: usize, height: usize, depth: usize, dx: f64) -> Self {
        let depth = depth.max(1);
        Self {
            width,
            height,
            depth,
            dx,
            cells: vec![0.0; width * height * depth],
        }
    }

    pub fn cells(&self) -> &[f64] {
        &self.cells
    }

    /// The cell value at a linear `[k][j][i]` index.
    pub fn value_linear(&self, idx: usize) -> f64 {
        self.cells[idx]
    }

    /// Writes the cell at a linear `[k][j][i]` index.
    pub fn set_linear(&mut self, idx: usize, value: f64) {
        self.cells[idx] = value;
    }

    #[inline]
    fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.height + j) * self.width + i
    }

    /// The cell value at `(i, j, k)`.
    pub fn value3(&self, i: usize, j: usize, k: usize) -> f64 {
        self.cells[self.index(i, j, k)]
    }

    /// Writes the cell at `(i, j, k)`.
    pub fn set3(&mut self, i: usize, j: usize, k: usize, value: f64) {
        let idx = self.index(i, j, k);
        self.cells[idx] = value;
    }

    /// The cell value at `(i, j)` of a 2D (`depth = 1`) field.
    pub fn value(&self, i: usize, j: usize) -> f64 {
        self.value3(i, j, 0)
    }

    /// Writes the cell at `(i, j)` of a 2D (`depth = 1`) field.
    pub fn set(&mut self, i: usize, j: usize, value: f64) {
        self.set3(i, j, 0, value);
    }

    pub fn total(&self) -> f64 {
        self.cells.iter().sum()
    }

    /// The 7-point Laplacian at `(i, j, k)`, with a zero-flux (Neumann)
    /// boundary (a 5-point stencil when `depth = 1`).
    pub fn laplacian3(&self, i: usize, j: usize, k: usize) -> f64 {
        let (w, h, d) = (self.width, self.height, self.depth);
        let (i, j, k) = (i as isize, j as isize, k as isize);
        let center = self.cells[self.index(i as usize, j as usize, k as usize)];
        let at = |x: isize, y: isize, z: isize| -> f64 {
            let (x, y, z) = (x as usize, y as usize, z as usize);
            self.cells[self.index(x, y, z)]
        };
        let left = if i > 0 { at(i - 1, j, k) } else { center };
        let right = if (i as usize) + 1 < w {
            at(i + 1, j, k)
        } else {
            center
        };
        let up = if j > 0 { at(i, j - 1, k) } else { center };
        let down = if (j as usize) + 1 < h {
            at(i, j + 1, k)
        } else {
            center
        };
        let back = if k > 0 { at(i, j, k - 1) } else { center };
        let front = if (k as usize) + 1 < d {
            at(i, j, k + 1)
        } else {
            center
        };
        (left + right + up + down + back + front - 6.0 * center) / (self.dx * self.dx)
    }

    /// The Laplacian at `(i, j)` of a 2D (`depth = 1`) field.
    pub fn laplacian(&self, i: usize, j: usize) -> f64 {
        self.laplacian3(i, j, 0)
    }

    /// One Gauss–Seidel relaxation pass solving the Poisson equation
    /// `∇²φ = source · scale` (`fixed` marks Dirichlet cells held constant).
    /// This is the domain-neutral operator an electromagnetics or gravitation
    /// domain uses to find a steady-state potential.
    pub fn poisson_step(&mut self, source: &[f64], scale: f64, fixed: &[bool]) {
        let (w, h, d) = (self.width, self.height, self.depth);
        let dx2 = self.dx * self.dx;
        for k in 0..d {
            for j in 0..h {
                for i in 0..w {
                    let idx = self.index(i, j, k);
                    if fixed[idx] {
                        continue;
                    }
                    let center = self.cells[idx];
                    let neighbor = |x: usize, y: usize, z: usize| self.cells[self.index(x, y, z)];
                    let left = if i > 0 { neighbor(i - 1, j, k) } else { center };
                    let right = if i + 1 < w {
                        neighbor(i + 1, j, k)
                    } else {
                        center
                    };
                    let up = if j > 0 { neighbor(i, j - 1, k) } else { center };
                    let down = if j + 1 < h {
                        neighbor(i, j + 1, k)
                    } else {
                        center
                    };
                    let back = if k > 0 { neighbor(i, j, k - 1) } else { center };
                    let front = if k + 1 < d {
                        neighbor(i, j, k + 1)
                    } else {
                        center
                    };
                    // ∇²φ = source·scale  =>  φ = (Σneighbours − source·scale·dx²)/6.
                    // A 2D slice's two off-edge z-terms equal the centre, so the
                    // fixed point matches the 5-point form.
                    self.cells[idx] =
                        (left + right + up + down + back + front - source[idx] * scale * dx2) / 6.0;
                }
            }
        }
    }

    /// Explicit forward-Euler diffusion `∂c/∂t = D·∇²c`, stable for small dt.
    /// A chemistry (reaction–diffusion) or temperature domain uses this.
    pub fn diffusion_step(&mut self, dt: f64, diffusivity: f64) {
        let (w, h) = (self.width, self.height);
        let lap: Vec<f64> = (0..self.cells.len())
            .map(|idx| {
                let i = idx % w;
                let j = (idx / w) % h;
                let k = idx / (w * h);
                self.laplacian3(i, j, k)
            })
            .collect();
        for (cell, &l) in self.cells.iter_mut().zip(&lap) {
            *cell += dt * diffusivity * l;
        }
    }

    /// Deterministic content hash (RFC-0025-compatible replay identity). The
    /// depth is implied by `cells.len()` given `width` and `height`.
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
        // phi(x) = x  =>  d2phi/dx2 = 0 (flat in y, z).
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

    #[test]
    fn laplacian_of_linear_3d_field_is_zero() {
        // phi(x, y, z) = x + 2y + 3z  =>  ∇²phi = 0.
        let mut f = Field::new3(6, 6, 6, 1.0);
        for k in 0..6 {
            for j in 0..6 {
                for i in 0..6 {
                    f.set3(i, j, k, i as f64 + 2.0 * j as f64 + 3.0 * k as f64);
                }
            }
        }
        assert!(f.laplacian3(3, 3, 3).abs() < 1e-12);
        // The 3D grid's centre is a local minimum for a positive bump.
        let mut g = Field::new3(5, 5, 5, 1.0);
        g.set3(2, 2, 2, 1.0);
        assert!(g.laplacian3(2, 2, 2) < 0.0);
    }

    #[test]
    fn depth_is_independent_and_round_trips_through_cells() {
        let mut f = Field::new3(2, 3, 4, 1.0);
        assert_eq!(f.cells().len(), 24);
        f.set3(1, 2, 3, 7.0);
        assert_eq!(f.value3(1, 2, 3), 7.0);
        assert_eq!(f.value3(0, 0, 0), 0.0);
        // Distinct depths produce distinct content hashes.
        assert_ne!(
            Field::new3(4, 4, 1, 1.0).hash(),
            Field::new3(4, 4, 2, 1.0).hash()
        );
    }
}
