# RFC-0039: Constraint joints (position-based family) and soft bodies
**Status:** Normative. The language gains a pairwise **constraint** stage, solved by deterministic position relaxation (mirroring the reference `PhysicsSystem` constraint stage), and **soft bodies** built from the same constraints. Joints act on the EIR component surface (`transform` positions, `velocity`, `rigid_body` mass), so they compose with `gravity`/`integrate`/`ground_contact` and remain byte-identical across the interpreter and JIT.

## Joints
```
systems {
  joint { on = a; other = b; type = distance;  length = 1.0; stiffness = 1.0 }
  joint { on = a; other = b; type = spring;    length = 1.0; stiffness = 20; damping = 0.5 }
  joint { on = a; other = b; type = weld;      stiffness = 1.0 }
  joint { on = a; other = b; type = hinge;     anchor = (0,0,0) }
  joint { on = a; other = b; type = ball;      anchor = (0,0,0) }
  joint { on = a; other = b; type = prismatic; axis = (1,0,0); limit = (0.0, 2.0) }
  iterations = 8
}
```

Rules:
1. **Lowering.** A `joint` system lowers once, for entity `on = a`. Each iteration reads the current positions of `a` and `b` (cross-entity `ReadView`), computes a correction, and writes both back (`WriteView`). `iterations` (default 4) repeats the sweep; `on = a; other = b`.
2. **Mass weighting.** Each side moves by `inv_mass = is_dynamic ? 1/mass : 0`, split by `inv_a / (inv_a + inv_b)`; a joint with `inv_a + inv_b == 0` is a no-op. Jointed bodies must declare `mass` (or `dynamic`) so a `rigid_body` exists.
3. **Kinds** (positional):
   * `distance` — keep `|a − b| = length`; `stiffness` scales the correction.
   * `spring` — as `distance` plus velocity damping along the axis (`damping`).
   * `weld` / `hinge` / `revolute` / `ball` / `spherical` — coincide the anchor points (`pos_a + anchor` and `pos_b + anchor`). In this point-mass engine these differ only in the rotational DOF, which is not simulated; they are positionally identical.
   * `prismatic` / `slider` — keep `b` on the line through `a` along `axis`; `limit = (lo, hi)` clamps the along-axis separation.
4. **Angular joints are rejected.** `cone`, `twist`, `universal`, `gear`, `rack`, `pulley`, and motors enforce rotational or ratio constraints; the engine has no rotational state, so they fail with a clear diagnostic rather than silently under-constraining.
5. **Determinism.** All reads/writes are existing opcodes; the solver is a fixed-order sweep, so results are reproducible and identical on both backends.

## Soft bodies
Implemented in RFC-0040 on top of `distance`/`spring` constraints.
