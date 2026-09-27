# RFC-0043: Value types — `int`/`bool` lowered to EIR
**Status:** Proposed. This RFC specifies how the language's value kinds
(`f64`, `i64`/`i32`/`u64`/`u32`, `bool`) are **lowered to typed EIR registers**,
instead of today's single `f64` register file with surface-only checks.

## Motivation

The EIR value model already carries `I32/U32/I64/U64/F32/F64/Bool`
(`reference/src/eir.rs`, `enum ValueType`) and the interpreter's `arith`/
`divrem`/`compare` are width- and kind-correct. The surface language, however,
widens everything to `f64`: comparisons lower to a `Bool` opcode but are then
re-widened by `truthy()`; integer literals become `f64`; and `let`-type
annotations (detail 89) are syntax-only. This wastes the typed core, loses exact
integer arithmetic (counts/indices), and makes `int`/`bool` second-class.

## Design

### Type inference (surface)

`expr_kind(&Expr) -> Kind` where `Kind ∈ {F64, Int, Bool, Unknown}`:
- integer literals (`7`, no decimal point) → `Int`; decimal (`7.0`) → `F64`;
- `+ - * / %` → `Int` iff both operands are `Int`, else `F64`;
- comparisons and `and`/`or`/`not` → `Bool`;
- casts (`i64(…)` → `Int`, `f64(…)` → `F64`, `bool(…)` → `Bool`);
- a bare name/slot/ref/call → `Unknown` unless a `let`-tracked local establishes
  its kind (the checker tracks `let` kinds in declaration order).

### Lowering

`lower_expr(expr, …) -> (u32, ValueType)` (the existing `u32`-returning
`lower_expr` becomes a wrapper that **coerces to F64** via the conversion
opcodes below, so existing call sites are unchanged):
- `Int` nodes emit `Const` with `Immediate::I64` and `I64` arithmetic opcodes;
  `Div`/`Rem` on `I64` trap on zero (RFC-0021) and divide toward zero;
- `Bool` nodes emit `Bool` typed results (`Cmp`, `Ne`, `Select` with `Bool`);
- state writes (`WriteView`, state slots are `f64`) and function arguments
  declared `f64` coerce `Int→F64`; a `Bool` written to state coerces to `1/0`;
- `Unknown` operands coerce to `F64` (today's behavior).

### New opcodes

Two conversions, added via the `declare_opcodes!` table (single source):
- `I64ToF64 = 233`, `F64ToI64 = 234` (convert width/kind; `F64ToI64` truncates
  toward zero and traps on non-finite input, detail 18).

Both are pure (no effect bits); the interpreter is the oracle and the JIT/AOT
must remain byte-identical (`step_cross`).

### Diagnostics

- detail 89 (type mismatch) uses `expr_kind` with `let`-kind tracking, so
  integer annotations become meaningful (not vacuous) and bool aliases pass.
- A `Bool` used where a number is required (and vice-versa) is detail 89.

## Validation

- Conformance: a cross-backend case asserting `Int`/`Bool` registers produce
  byte-identical writes and that `I64ToF64`/`F64ToI64` round-trip.
- Property tests over the typed arithmetic opcodes.
- Equivalence: every program that compiles under today's `f64`-only lowering
  and does not use integer literals in ambiguous `/` positions yields identical
  results.

## Backward compatibility

Source-compatible: `1.0`-style literals are unchanged; `2` (no decimal) becomes
`Int` and coerces where a number is required, so existing programs keep working.
The one behavioral edge — `1 / 2` (both integer literals) now yields `0` instead
of `0.5` — is intentional (integer division) and documented.

## Alternatives

- **Keep `f64`-only** (status quo): simple, but forfeits the typed core and
  exact integer semantics. Rejected as the long-term shape.
- **Auto-narrow every integer literal to `Int`** without `let` tracking:
  simpler but makes `1 / 2` a surprising `0` in more places. The `let`-tracked
  variant is safer and is what this RFC specifies.
