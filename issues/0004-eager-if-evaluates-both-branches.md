# 0004 — `if(c,a,b)` evaluates both branches (Medium)

## Summary

The expression-level conditional is lowered eagerly: both branches are computed
into registers, then `Select` picks one. Branch code therefore runs
unconditionally — including division (a defined trap in the EIR contract) and
side-effecting calls (`print`, `fset`, `emit`).

## Evidence

- Lowering: `reference/src/lang/mod.rs:1510-1525` — lowers `args[0]`, `args[1]`,
  `args[2]` in order, then emits `Opcode::Select(cond, a, b)`. No branch
  skipping.
- Known in the grammar: `reference/src/lang/parser.rs:86` — comment contrasts
  the lazy `if cond { return } else { return }` form in `funcs` "unlike the
  eager `if(c,a,b)` expression, which evaluates both branches".
- Documented: `docs/lang-usage.md:166-171` — "The expression `if(c, a, b)`
  evaluates *both* branches, so it cannot be used for recursion."
- Div-by-zero is a defined trap in the EIR contract (RFC semantics; AGENTS.md
  §4 baseline).

## Impact

- `if(c, 1.0/x, 0.0)` traps when `c` is false.
- `if(c, fset(h, i, j, 1.0), fget(h, i, j))` writes unconditionally — in L4's
  field tutorial the guarded write pattern is exactly what users will reach for
  (`docs/lang-usage.md:262`).
- Users coming from any lazily-conditional language will assume guarding works.

## Fix

1. Document the trap interaction explicitly in the pitfalls table
   (`docs/lang-usage.md:455-469`) and next to the builtin list (`:630`).
2. Consider an eager-safe form: short-circuit at EIR level via
   `Branch`/`Select`-with-guard for pure operands, or reject `Div`/`Rem` with a
   zero constant under an eager `if` (static check).
3. At minimum, add a `safe_div(x, y)` builtin so guarded division has a
   non-trapping idiom.

## Labels

language-design, traps, medium
