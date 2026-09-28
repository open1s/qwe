# PWE issues

Findings from two passes: (1) a language-design review of `lang.pest`,
`reference/src/lang/*`, `docs/lang-usage.md`, and the READMEs (0001–0010);
(2) a black-box testing pass with compile/run probes (0011–0017). Every
finding carries `file:line` evidence and a reproduction. All are filed
upstream at github.com/open1s/qwe/issues. (3) a verification pass against
the developer fix commit `1df656bb`: 0001–0015 verified fixed except
0011 (reopened: `on`-less `send`/`recv` still corrupts, docs omit `on`) and
the 0003 residual, filed as 0018; 0016 verified; 0017 still open by design.
(4) a deep robustness/diagnostics/docs-conformance battery (0019–0026):
283 probes across syntax/semantics/CLI with 0 panics — robustness passes;
the findings below are diagnostics, semantic traps, and docs drift, all
re-verified on main `9d6277fe`; (5) a review of the typed-`let` commit
`b682e692` (0027–0028, re-verified on that commit); (6) fix-verification
of `4483cf46`/`c7ef2a65`/`9bfc38b4`/`8b892c1c` on main `8b892c1c` —
#11/#17/#18/#19/#21/#23/#24/#27 verified fixed and left closed; #6/#20/
#22/#25/#26/#28 reopened as partial with evidence comments; #29 filed
(the #7 fix's blockquote breaks the parameter table); (7) fix-verification
of `ea8caf6e` (closures of 0006/0020/0022/0025/0026/0028/0029) on main
`ea8caf6e` — #20/#22/#25/#26/#29 verified fixed and left closed (gates
green: fmt, clippy, 308 tests, conformance 18/18); #6/#28 reopened again
as partial with evidence comments; (8) a review of the native backend
commit `7cc2d5a5` — gates green at the commit (fmt, clippy, 311 tests,
conformance 18/18), but probe-verified semantic/crash findings filed as
0030–0034 and an RFC-0027 contract finding as 0035.

| # | Severity | Title | File |
| --- | --- | --- | --- |
| [0001](https://github.com/open1s/qwe/issues/1) | High | README teaches `slot = expr` as Euler integration | README.md:150 |
| [0002](https://github.com/open1s/qwe/issues/2) | High | Unknown identifiers/refs silently read 0.0, no diagnostic | reference/src/lang/mod.rs:1316 |
| [0003](https://github.com/open1s/qwe/issues/3) | High | Param-vs-rule dispatch depends on system kind and literal RHS | reference/src/lang/parser.rs:159,269 |
| [0004](https://github.com/open1s/qwe/issues/4) | Medium | `if(c,a,b)` evaluates both branches (traps + side effects) | reference/src/lang/mod.rs:1510 |
| [0005](https://github.com/open1s/qwe/issues/5) | Medium | Doc drift: soft bodies/joints shipped but README says absent | README.md:270 |
| [0006](https://github.com/open1s/qwe/issues/6) | Medium | Magic slot conventions (nbody mass=slot 6, slots 7/8/9) | docs/lang-usage.md:233,695 |
| [0007](https://github.com/open1s/qwe/issues/7) | Medium | Channels are a last-writer-wins mailbox, not Go-style channels | reference/src/lang/mod.rs:3206 |
| [0008](https://github.com/open1s/qwe/issues/8) | Low | Reserved-word table omits `else`, `inte`, `deriv`, `+=` | docs/lang-usage.md:517 |
| [0009](https://github.com/open1s/qwe/issues/9) | Low | Malformed unit annotations silently disable checking | reference/src/lang/parser.rs:287 |
| [0010](https://github.com/open1s/qwe/issues/10) | Low | `print(x)` is a capped debug side-channel, undocumented as such | reference/src/eir.rs:807,1870 |
| [0011](https://github.com/open1s/qwe/issues/11) | High | `send`/`recv` run for every entity, silently ignore `on` | reference/src/lang/compile.rs:103 |
| [0012](https://github.com/open1s/qwe/issues/12) | High | `params { … [unit] }` units never recorded → checks skipped | reference/src/lang/parser.rs:1089 |
| [0013](https://github.com/open1s/qwe/issues/13) | Medium | Side-effect-only `update` (`let`/bare call) rejected, error 55 | reference/src/lang/compile.rs:216 |
| [0014](https://github.com/open1s/qwe/issues/14) | Medium | `pwe run` claims cross-check "every step", actually 1-in-16 | cli/src/main.rs:575 |
| [0015](https://github.com/open1s/qwe/issues/15) | Low | `pwe run` reports the wrong failing step (batch start) | cli/src/main.rs:402 |
| [0016](https://github.com/open1s/qwe/issues/16) | Low | Most detail-77 errors print without source location | reference/src/lang/compile.rs:1408 |
| [0017](https://github.com/open1s/qwe/issues/17) | Medium | F64 ÷0 is silent inf/NaN vs RFC-0021 "defined trap" | reference/src/eir.rs:2503 |
| [0018](https://github.com/open1s/qwe/issues/18) | Low | Slot named `dt` unwritable; rule silently rebinds the timestep | reference/src/lang/parser.rs |
| [0019](https://github.com/open1s/qwe/issues/19) | High | `schedule()` documented builtin never compiles (always detail 6) | reference/src/lang/lower.rs:547 |
| [0020](https://github.com/open1s/qwe/issues/20) | Medium | EIR validation errors leak as bogus line-1 carets, "unspecified compile error" | reference/src/eir.rs:13,1075 |
| [0021](https://github.com/open1s/qwe/issues/21) | Medium | `pwe run` drops the rich diagnostics pushed for details 88/69 | cli/src/main.rs:476,530 |
| [0022](https://github.com/open1s/qwe/issues/22) | Medium | User `funcs` calls never arity-checked (silent extra args / runtime 17) | reference/src/lang/lower.rs:394 |
| [0023](https://github.com/open1s/qwe/issues/23) | High | `rk4` silently drops plain assignments — wrong physics, 55 advice loop | reference/src/lang/compile.rs:286 |
| [0024](https://github.com/open1s/qwe/issues/24) | Medium | Slot-index bounds bypassed by assign path; OOB reads silently 0.0 | reference/src/lang/compile.rs:210 |
| [0025](https://github.com/open1s/qwe/issues/25) | Low | Docs conformance batch (L1 number, Appendix B, section order, color) | docs/lang-usage.md:149 |
| [0026](https://github.com/open1s/qwe/issues/26) | Low | Misleading param diagnostics (watch `into`, send `chan`) | reference/src/lang/compile.rs:124 |
| [0027](https://github.com/open1s/qwe/issues/27) | Medium | Annotated `let` in loop bodies swallowed: initializer replaced by type name, detail 89 bypassed | reference/src/lang/parser.rs:444 |
| [0028](https://github.com/open1s/qwe/issues/28) | Low | `let` type check never types names: bool alias wrongly rejected (89), integer annotations vacuous | reference/src/lang/parser.rs:81 |
| [0029](https://github.com/open1s/qwe/issues/29) | Low | Mailbox blockquote wedged mid-table breaks the system-parameter table rendering | docs/lang-usage.md:603 |
| [0030](https://github.com/open1s/qwe/issues/30) | High | `NativeProgram::call` has no arity validation: safe API SIGSEGVs on wrong-length args | reference/src/native.rs:181 |
| [0031](https://github.com/open1s/qwe/issues/31) | High | Same-process recompile silently executes the first program's code (fixed temp path + dlopen cache) | reference/src/native.rs:132 |
| [0032](https://github.com/open1s/qwe/issues/32) | Medium | Native f64 Div/Rem by zero returns inf/NaN instead of the RFC-0021 trap (interpreter detail 18) | reference/src/native.rs:241 |
| [0033](https://github.com/open1s/qwe/issues/33) | Medium | Native `Sign` diverges from interpreter `signum` at 0.0, −0.0, NaN | reference/src/native.rs:317 |
| [0034](https://github.com/open1s/qwe/issues/34) | Medium | Native CondBr/Select conditions: IEEE `!= 0.0` vs interpreter bit-test (`-0.0` diverges); differential-test coverage gap | reference/src/native.rs:326 |
| [0035](https://github.com/open1s/qwe/issues/35) | Medium | Native backend bypasses RFC-0027 normative lifecycle (no Validate/CapabilityCheck/manifest/artifact identity) | reference/src/native.rs:122 |
| [0036](https://github.com/open1s/qwe/issues/36) | Medium | f64 comparisons with NaN: interpreter traps EirInvalid 18, native returns IEEE result — tier divergence, semantics unspecified | reference/src/eir.rs:2776 |

## Suggested order

1. 0011 + 0012 — silent state corruption / silent loss of dimensional
   checking; both have one-line root causes.
2. 0001 — actively misleading, one-line fix; 0014 — headline guarantee not
   delivered by the CLI.
3. 0002 + 0009 (see also 0012) — one new detail code each; removes a class of
   silent wrongness.
4. 0003 + 0006 — name/layout validation with diagnostics; 0013 + 0016 —
   compiler/diagnostic consistency.
5. 0015, 0017 — CLI honesty / RFC contract decision.
6. 0004, 0007, 0008, 0010 — documentation passes; 0005 — stale-limits sweep
   (include wiki FAQ and `docs/book/faq.md`).

Deep-battery batch (0019–0026):

1. 0023 — silent wrong physics in the accuracy integrator; decide
   assign-in-rk4 semantics before anything else touches rk4.
2. 0019 — fully documented builtin is dead code; needs a contract decision
   (yields nothing vs yields a handle), then a test.
3. 0020 + 0021 — the two diagnostic pipelines (EIR compile errors, runtime
   step failures) both throw away their messages; one fix each restores
   every detail code that flows through them.
4. 0022 + 0024 — arity and bounds checks exist but are wired to only one
   path each; small, testable extensions of 52/59/85.
5. 0025 + 0026 — docs sweep and message wording; can ride along with any
   touch of the cited files.

Typed-`let` review batch (0027–0028):

1. 0027 — the new feature is broken in loop bodies (silent wrong value +
   check bypassed); one `parse_let_parts` call fixes it, needs loop/funcs
   regression tests.
2. 0028 — binding-aware annotation checking, `type_name` word boundary,
   integer-annotation honesty; can follow 0027.

Fix-verification pass 1 (reopen reasons, pass 6):

1. 0020 — `error 60: internal EIR validation failed (EIR detail 4)` for
   `sin(1.0,2.0)`: wrong code, no real reason, builtin arity still
   reaches the validator.
2. 0022 — arity check misses system `let` statements; message has no
   name/counts/location.
3. 0006, 0025, 0026, 0028 — each fix landed only part of the issue's
   fix list (details in the reopen comments).
4. 0029 — move the mailbox note below the parameter table.

Fix-verification pass 2 (`ea8caf6e`, pass 7 — closures of 0006/0020/0022/
0025/0026/0028/0029):

1. Verified fixed, left closed: 0020 (builtin arity checked at parse →
   `error 59` with the function name, unknown names → clear 59 instead of
   EIR detail 6, `Invalid (NN)` → `error NN` unified), 0022 (name +
   expected/got counts, system-`let` walk gap closed, `send value` covered),
   0025 (color → 64 with explicit message, section order documented,
   `ds/dt = A·s + c`), 0026 (watch wrong-kind → 48 with caret), 0029
   (note moved below the table; GitHub render = 1 table, all rows intact).
   Gates green: fmt, clippy, 308 tests, conformance 18/18.
2. Reopened as partial: 0006 (91/92/95 + suppression with `orient=true`
   + checklist line verified — but non-nbody rules writing slots 7/8/9
   stay silent and "vice versa" is not covered); 0028 (`boolean`→89,
   bool-as-number→89 with caret, aliasing OK — but `let i: i64 = 1.5`
   still compiles and evaluates to 1.5).
3. Minor residuals accepted (not reopened): arity errors still compile
   with byte offset 0 (no caret), wrong-section-order parse error still
   names the grammar rule (order is now documented).

Native-backend review batch (0030–0035, commit `7cc2d5a5`):

1. 0030 + 0031 — safety/staleness first: arity guard on the safe
   `call()` API; unique hash-keyed artifact path + cleanup (also
   delivers the artifact-identity part of 0035).
2. 0032 + 0033 + 0034 — interpreter equivalence: trap channel or
   `Div`/`Rem` exclusion, exact `sign` semantics, bit-level condition
   tests; grow the differential test into an opcode × input-class
   matrix (zeros, `±0.0`, NaN, inf, branches).
3. 0035 — settle RFC-0027 conformance (implement the lifecycle or
   amend the RFC deliberately) before wiring the backend into any
   production entry point.

Fix-verification pass 3 (`bb7cf78a`, pass 9 — closures of 0030–0035 +
re-verification of the pass-7 reopens):

1. Verified fixed, left closed: 0030 (arity guard — `call(&[])`,
   too-many and unknown-id now `Err(Invalid,5)`, exit 0, no SIGSEGV;
   correct arity returns the right value), 0031 (artifact dir keyed by
   content-hash + per-process seq: `n1.call(10)=11`, `n2.call(10)=12`,
   hashes differ, dirs removed on Drop — zero leftovers after exit),
   0032 (div/rem by ±0.0 → native `Err EirInvalid 18` == interpreter,
   via the new `div_guard` C-ABI shim), 0033 (`sign(0.0)=1`,
   `sign(-0.0)=-1`, `sign(NaN)=NaN`, bit-identical to `signum`), 0034
   (`Select`/`CondBr` now use the bit-test `pwe_truthy` — probes for
   0.0/−0.0/1.0/−2.5/NaN all MATCH on both constructs), 0035 (validate +
   dominance check before emit, hash-keyed identity, RFC-0027 addendum
   recording the in-process-kernel exemption deliberately, backend still
   not wired into any production entry point, roadmap updated). Gates
   green: fmt, clippy, 314 tests, conformance 18/18.
2. Re-verified pass-7 reopens against `09cc6cd8`, staying closed: 0006
   (warning 96 both ways — rule writes slot 7 without `orient = true`;
   `orient = true` with only 8 slots; correct 9-slot `orient = true`
   case stays silent), 0028 (`let i: i64 = 1.5` → `error 89: … the
   expression is not an integer constant`, rc=1).
3. Filed 0036 from the verification probes: f64 comparisons with NaN —
   interpreter traps `EirInvalid 18` (`compare()` → `partial_cmp`,
   eir.rs:2776) while native returns the IEEE result; every
   `if <cond>` is reachable through `truthy()`'s `Ne(reg, 0.0)`
   (lower.rs:947); no RFC defines the semantics; the differential
   matrix has NaN inputs but no comparison bodies.

Local copies of the bodies live next to this file (`0001-…` … `0035-…`).
