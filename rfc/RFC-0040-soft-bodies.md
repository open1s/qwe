# RFC-0040: Soft bodies (mass-spring grids)
**Status:** Normative. The language gains **soft bodies**: an `nx × ny` grid of dynamic particles connected by spring constraints, solved by the same deterministic, mass-weighted position relaxation as joints (RFC-0039). Soft bodies act on the EIR component surface (`transform` positions, `velocity`, `rigid_body` mass) and reuse only existing opcodes, so they compose with `gravity`/`integrate` and remain byte-identical across the interpreter and JIT.

## Declaration
```
world {
  soft cloth {
    nx = 8; ny = 8
    spacing = 0.25
    origin = (0, 4, 0)
    mass = 0.2
    shape = sphere; size = 0.08
  }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  soft      { body = cloth; stiffness = 1.0; damping = 0.5; iterations = 6 }
}
```

Fields (all optional except a sensible default): `nx` (default 8), `ny` (default 8), `spacing` (1.0), `origin` (0,0,0), `mass` (1.0), and the presentation `shape`/`size` applied to every particle.

## Rules
1. **Particles.** A `soft` declaration creates `nx × ny` **active dynamic** entities, ids contiguous and immediately after the declared entities, channels, and pools (matching `build_scene`); particle `k = j·nx + i` is named `<body>#<k>` and starts at `origin + (i·spacing, j·spacing, 0)` with `mass` and a zero velocity.
2. **Constraints.** The grid generates:
   * **structural** edges to the right and up (`spacing`);
   * **shear** edges on both diagonals (`spacing·√2`);
   * **bend** edges two cells right/up (`2·spacing`).
   Each is a distance constraint `|p_a − p_b| = rest`.
3. **Lowering.** A `soft` system lowers one function per particle. The particle owning a constraint's **lower index** applies it; per iteration it applies every such constraint as a position-relaxation pass (the RFC-0039 distance math, mass-weighted by `is_dynamic ? 1/mass : 0`), then the next particle's function applies its own. `iterations` (default 4) repeats the per-particle sweep; `stiffness` and optional `damping` (along-axis relative velocity) parameterize the springs.
4. **Determinism.** The sweep order is fixed (particle id, then constraint order, then iterations); every read/write is an existing opcode, so results are reproducible and identical on both backends. No new opcode, ABI, schema, protocol, snapshot, or transaction contract is introduced.
5. **Presentation.** Particles render as their `shape`; the structural and shear edges are emitted as `PresentationFrame.bonds`, so the viewer draws the mesh.
6. **Scope.** This is a 2D lattice (`nz = 1`) of point masses, so it models cloth/membrane-like sheets. Volumetric (3D) grids, bending stiffness beyond the two-away edges, volume/pressure constraints, and self-collision are out of scope; `soft` complements `joint`/`pool` and does not replace a full FEM solver.

Implementation is staged: **Stage 1** adds the `soft` declaration, particle layout, and the `soft` system (structural/shear/bend, position relaxation). **Stage 2** adds render bonds and the example/`docs`. **Stage 3** re-baselines conformance and the example programs.
