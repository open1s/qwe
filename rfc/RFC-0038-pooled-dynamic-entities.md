# RFC-0038: Pooled Dynamic Entities (spawn / despawn / active)
**Status:** Normative. A world MAY declare an **entity pool** — a fixed, contiguous block of pre-allocated entity slots that start **inactive** and are brought into and out of existence at runtime. The pool keeps the EIR static (one function per entity, as required by RFC-0019/0020): dynamic behaviour is activation, not allocation. An inactive slot is part of world state but is skipped by user systems and hidden in the render view.

Declarations:

```
world {
  entity probe { state = (x = 0) }
  pool particles[64] { state = (x = 0, vx = 0) }   # 64 inactive slots
}
systems {
  spawn   { on = probe; pool = particles }          # at most one free slot per caller/step
  despawn { on = particles; when = state.x > 10 }   # deactivate matching slots
  update  { on = particles; dt = 0.1 vx = -0.5 }    # runs only on active slots
}
```

Rules:
1. **Layout.** Pool slots are assigned ids in declaration order, contiguously, immediately after the ordinary declared entities; slot `k` of a pool declared after `B` ordinary entities and `P` earlier slots has id `B + P + k + 1`. The complete entity list (ordinary + slots) is the program's static entity set.
2. **Active flag.** Every `Entity` has an `active: bool` (reference scene state; ordinary entities default `true`, pool slots `false`). It is canonical component `active_id()`, is readable/writable like any component, and is part of world state (`state_hash`/snapshot/serialize).
3. **Skip.** The per-entity function of every user system whose target is a pool slot begins with `if !active(e) { return }`; grid-field and `spawn` systems are unaffected. Physics integration and neighbouring queries likewise skip inactive slots.
4. **`spawn`.** Per caller and per step, at most one slot is activated, the lowest-id inactive slot in the pool; with no free slot the step is a no-op. `spawn` copies the caller's state slots of matching arity into the slot. It lowers to the deterministic opcode `FindFreeSlot` (operands: the four little-endian `u32` limbs of the pool's first slot id, then the slot count), which returns the first inactive slot id or `0` when full, followed by masked writes (`Select` on `slot>0`), so no branches are needed and only existing opcodes plus `FindFreeSlot` are used.
5. **`despawn`.** Deactivates every slot of the pool for which `active` holds and `when` is true, in ascending slot order. Lowered as masked writes to `active`: `active(slot) = Select(active(slot) && when, 0, active(slot))`.
6. **`active(slot)`.** The intrinsic reads rule 2; it is `1.0` for active, `0.0` otherwise.
7. **Presentation.** Inactive slots are omitted from `PresentationFrame` (like hidden fields).
8. **Determinism.** All mutations are masked writes over existing opcodes plus the pure `FindFreeSlot` scan; the interpreter and JIT remain byte-identical and the cross-backend check is unchanged. `FindFreeSlot` is additive and target-neutral; no frozen ABI/schema/snapshot format is broken (the new `active` byte is covered by the versioned snapshot/state hash).

Implemented, both stages landed: **Stage 1** added the `active` flag, `active_id()`, runtime read/write, snapshot/state-hash coverage, present hiding, and the `FindFreeSlot`/`SpawnInto` opcodes (runtime-resolved pool spawn). **Stage 2** added the `pool` declaration, slot layout, the `active` guard (SSA- and branch-target-shifting prologue), and the `spawn`/`despawn`/`active` language surface, plus `cli/examples/particles.pwe`. `FindFreeSlot`/`SpawnInto` are additive reference opcodes (227/228).
