# 0017 — F64 divide-by-zero yields silent inf/NaN while RFC-0021 says divide-by-zero is a defined trap (Medium)

## Summary

`rfc/RFC-0021-eir-binary.md:25` states:

> Integer arithmetic wraps at width. **Divide-by-zero**, invalid
> shift/load/capability, and `TRAP` are **defined traps**. … Interpreter is
> semantic oracle; JIT/AOT preserve result, transaction, event order, and
> traps for identical inputs.

The reference interpreter implements the trap **only for integer**
division/remainder (`divrem` returns `None` → `EirInvalid (18)`). F32/F64
division returns IEEE results unconditionally: `1.0/0.0 → inf`,
`0.0/0.0 → NaN`, the run succeeds, and `inf` propagates into entity state
with no diagnostic.

Since the PWE surface language is entirely `f64`, a PWE program can **never**
trigger the RFC's divide-by-zero trap — while the EIR code comment claims RFC
compliance. Either the RFC's wording must be scoped to integer division, or
the oracle must trap (or at least diagnose) F64 division by zero; today the
contract is ambiguous and the observable behavior is silent value corruption.

## Repro

```pwe
world { gravity = (0, 0, 0)
  entity e { state = (x = 0.0) }
}
systems {
  update { on = e; dt = 1.0  x = 1.0/0.0 }
}
```

```
$ pwe compile n1_div_zero.pwe -o n1.pweb && pwe run n1.pweb --steps 1
  ran 1 steps (interpreter == JIT, every step)
  #1   e   pos = (      inf, ...)  state = [inf]     ← exit 0, no trap
```

`x = 0.0/0.0` → `state = [NaN]`, likewise silent
(`_probe_out/batch6/n1_div_zero.pwe`, `n2_nan.pwe`).

## Evidence

- `rfc/RFC-0021-eir-binary.md:25` — "Divide-by-zero … are defined traps";
  the sentence follows the integer-wrapping sentence but does not explicitly
  scope divide-by-zero to integers, and the following sentence makes the
  interpreter the oracle for "traps for identical inputs".
- `reference/src/eir.rs:2472-2475` — `divrem` doc comment: "Division and
  remainder with defined traps (RFC-0021)".
- `reference/src/eir.rs:2477-2497` — integer arms return `None` on
  divide-by-zero/overflow → mapped to `EirInvalid (18)` at
  `reference/src/eir.rs:1315-1317`.
- `reference/src/eir.rs:2503-2504` —
  `(Opcode::Div, F32(a), F32(b)) => Some(F32(a / b))` and
  `(Opcode::Div, F64(a), F64(b)) => Some(F64(a / b))` — never `None`.
- Related latent oddity: `reference/src/eir.rs:1290` —
  `Opcode::Trap | Opcode::Unreachable => return Ok(())` makes an explicit
  `Trap` opcode *succeed* silently in the interpreter (no lang construct
  emits it today, but it contradicts "TRAP [is a] defined trap").

## Impact

- Silent `inf`/`NaN` propagation into world state (rendering, physics,
  comparisons all degrade without any diagnostic); mirrors the class of
  silent-wrongness reported in #2.
- RFC ↔ implementation disagreement is exactly the case AGENTS.md §16 says
  must be resolved deliberately (RFC → conformance tests → implementation).
- If the intent is IEEE behavior for floats, the RFC must say so, and issue
  #4's "eager `if` evaluates traps in dead branches" claim needs rewording
  accordingly (see comment on #4).

## Suggested fix

1. Decide the contract: (a) F64 div-by-zero is a defined trap (oracle traps,
   `EirInvalid`), or (b) IEEE semantics are intended — then amend RFC-0021 to
   scope "divide-by-zero" to integer ops and document `inf`/`NaN` as the F64
   result.
2. Whichever way, add a conformance test asserting the chosen behavior, and
   consider a compile-time or run-time diagnostic for literal `x/0.0`
   patterns.
