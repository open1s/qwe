# RFC-0037: Bulk Field Sweeps (dense field overlay + solver opcodes)
**Status:** Normative. A `SceneRuntime` keeps a **dense field overlay**: for each grid field it touches, a `Vec<f64>` of the cells, materialized lazily from the `Field` on first field read or write. Every grid read (`ReadFieldCell`, `FieldLaplacian`, the bulk opcodes below) and every grid write (`WriteFieldCell`, bulk opcodes) observes the overlay when present and the base `Field` otherwise. The grid is therefore **not** routed through the per-cell `pending` overlay (which stays for entity components), so a full-grid sweep costs O(cells) tight-loop work instead of O(cells·log cells). Grid writes are flushed from the overlay to the `Scene` once at commit; the flush is the sole path by which grid cells change.

Bulk solver opcodes lower a whole sweep to **one instruction** (target = the field's `ComponentRef`, as for `ReadFieldCell`; the field's kind/dims/dx come from the runtime's field table):

| Opcode | Operands (register ids) | Semantics |
| --- | --- | --- |
| `FIELD_DIFFUSE` | `rate` | One Jacobi sweep `T += rate·∇²T` over every cell (zero-flux), identical to the current `DiffuseSystem` lowering and to `Field::diffusion_step(dt=1)`. |
| `FIELD_WAVE` | `pid0..pid3`, `cfl`, `damping`, `absorb`, `absorb_width` | One leapfrog step of `u_tt = c²∇²u` (the current `WaveSystem` arithmetic, including the graded sponge), then `prev ← u`. `pid0..pid3` are the four little-endian `u32` limbs of the 128-bit `ComponentTypeId` of the `prev` field (a second component reference cannot fit an operand; `ComponentTypeId` is 128-bit). |
| `FIELD_POISSON` | `sid0..sid3`, `iters`, `scale` | `iters` in-place Gauss–Seidel sweeps of `∇²φ = ρ·scale` over the interior (boundary cells held). `sid0..sid3` are the limbs of the `source` field id; all-zero means "no source". |

Rules:
1. **One instruction, native loop.** The opcodes execute the sweep in the runtime with a native Rust loop; no per-cell EIR instructions and no per-cell `WorldWrite`s are emitted. The lowerer emits exactly one bulk opcode per `diffuse`/`wave`/`poisson` system.
2. **Determinism.** The arithmetic, cell order, and stencil are byte-identical to the current unrolled lowering (which is the differential baseline), so the interpreter and the JIT — both delegating to the same runtime method — remain byte-identical.
3. **Cross-backend equality.** `step_cross` additionally requires `rt_a.overlay == rt_b.overlay` for every field (byte equality of the dense overlays), since grid state no longer appears in the write list. Entity-component writes, events, and the event queue are compared as before.
4. **Commit.** After a successful cross-step, the interpreter's overlay is flushed into the authoritative `Scene` (grid cells), then the entity-component `WorldWrite`s are applied. `WorldVersion`/state-hash/snapshot semantics are unchanged: fields remain ordinary `Scene` state.
5. **Scalar grid access.** `fget`/`fset`/`flap` read and write the same dense overlay, so scalar edits and bulk sweeps in one step interleave with the documented read-after-write visibility.
6. **Compatibility.** Opcodes are additive; existing modules (per-cell lowering, codec, AOT artifacts) remain valid. `apply_writes` no longer mutates grid cells (the overlay flush owns that path); unknown/older runtimes that do not implement the bulk methods return `Unsupported` and cannot execute bulk-opcode modules — the reference interpreter is the semantic oracle.

Implementation is staged: **Stage 1** introduces the dense field overlay and routes scalar grid access through it (no opcode/contract change; `WorldWrite`s unchanged). **Stage 2** adds the three opcodes, the single-instruction lowering, the overlay equality check, and the flush. **Stage 3** re-baselines conformance and the examples.
