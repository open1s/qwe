# RFC-0027: JIT Contract
**Status:** Normative. Lifecycle is Compile→Validate→CapabilityCheck→Link→Publish→Execute, with Profile/Invalidate returning to Compile. Artifact identity is RFC-0035 identity plus EIR hash, target, Runtime ABI major, SchemaSetHash, feature and profile hashes. Assumptions (layouts/constants/ranges/device/alignment) are checked at entry. Publication and invalidation are atomic; execution deoptimizes only at declared EIR safe points and resumes generic semantics. No executable artifact runs without a capability manifest.

## Addendum — in-process native kernels (exemption)

**Status:** Normative. This addendum defines a **narrow, deliberate exemption**
from the full lifecycle above for the C-codegen native backend
(`reference/src/native.rs`, `NATIVE_TARGET = 3`). Like every other artifact it
still MUST share EIR semantics with the interpreter (the oracle) and produce
byte-identical reads/writes.

### What may be exempt

A native artifact is exempt only when **all** of the following hold:

1. It is compiled from an [`EirModule`] that has passed `validate(true)` and
   `verify_linear_dominance()` (**Validate** is never skipped).
2. Its functions carry **no effect bits** other than
   `EIR_EFFECT_READ_WORLD | EIR_EFFECT_WRITE_WORLD | EIR_EFFECT_BARRIER`
   (no time / random / io / atomic / device / network / resource effects).
3. It performs **no capability-bearing operation** other than reading/writing
   components of the world it is already running inside. Grid-field access,
   spatial queries, and device/network/resource access are not exempt.
4. It is **not reachable from a production entry point** (`pwe run` / `present` /
   `pwe-api`). It is used in-process by callers that already hold the world.

### What is skipped, and what replaces it

| Stage | Exempted? | Replacement |
| --- | --- | --- |
| Compile | no | `cc -O2 -ffp-contract=off` |
| Validate | **no** | `EirModule::validate` + `verify_linear_dominance` before emit |
| CapabilityCheck | **yes** | the exemption condition (3): no capability surface beyond the caller's own world |
| Link | no | `dlopen` (per-artifact) |
| Publish | **yes** | in-process only; never published to a shared cache |
| Execute | no | via the shim table (`PweCtx`), identical to interpreter semantics |

### Artifact identity

A native artifact MUST carry RFC-0035-style identity (SHA-256 of the emitted
source) and MUST key its on-disk path by that identity, so distinct programs
never share a loaded image and identical programs may deliberately cache-hit.

### Meaning-equivalence requirements

The differential conformance suite MUST cover the cases where IEEE and RFC-0021
differ, at least: division/remainder by `±0.0` (must trap with detail 18),
`signum` of `±0.0`/`NaN`, and bit-exact condition tests (condition truth is
`bits != 0`, so `-0.0` is **true**). Any new eligible opcode MUST be added to
the differential matrix in the same change.

Any integration that lifts condition (4) (wiring native code into a production
entry point) MUST first implement the full lifecycle (CapabilityCheck with a
manifest, Publish) or obtain a further addendum.
