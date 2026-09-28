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
| [0037](https://github.com/open1s/qwe/issues/37) | Medium | WGSL `Select` condition: IEEE `!= 0.0f` vs interpreter bit-test (`-0.0` flips); CPU oracle shares the bug | reference/src/wgsl.rs:234 |
| [0038](https://github.com/open1s/qwe/issues/38) | Medium | Emitted WGSL builtins diverge from the CPU oracle (sign/round/hypot/Rem); emitter never validated by `naga` despite the doc claim | reference/src/wgsl.rs:195 |
| [0039](https://github.com/open1s/qwe/issues/39) | Low | WGSL device backend never traps (div/rem-by-zero, NaN compares) while interpreter + native both trap EirInvalid 18 — tiers disagree; decision needed before wiring | reference/src/wgsl.rs:17 |
| [0040](https://github.com/open1s/qwe/issues/40) | Medium | Native JIT deopt re-executes a step on partially-committed native writes — trap swallowed, state double-applied (`enable_native_jit` + `step_jit`) | reference/src/native.rs:198 |
| [0041](https://github.com/open1s/qwe/issues/41) | Medium | GPU field-sweep backend is net-slower than the CPU at every tested size (per-step buffer alloc + full f64↔f32 conversion + sync readback; 2.55×/2.0×/1.34× slower at 0.26M/2.1M/16.8M cells on M2 Max) | reference/src/gpu.rs:91 |
| [0042](https://github.com/open1s/qwe/issues/42) | Low | `pwe run --gpu` without the backend reports "error 5: unspecified compile error" — feature/device/platform failures collapse into one misleading diagnostic | cli/src/main.rs:480 |
| [0043](https://github.com/open1s/qwe/issues/43) | Low | `Gpu::new` doc comment still cites "detail 5" after the 98/99 diagnostics split — public API doc contradicts the actual error contract | reference/src/gpu.rs:63 |
| [0044](https://github.com/open1s/qwe/issues/44) | Medium | `step_cross` bypasses the threaded strategy (interpreter side calls `execute_with_index` directly); commit/test/roadmap/ADR-0003 all claim it exercises threaded-vs-JIT, so `--threaded` state is never runtime cross-checked | reference/src/lang/runtime.rs:584 |
| [0045](https://github.com/open1s/qwe/issues/45) | Low | Threaded vs jump-table dispatchers report different diagnostics for the same failure: detail 16 vs 17 for missing operands; call errors 32/33/34 carry byte_offset 0 vs pc | reference/src/eir.rs (th_get / h_call) |
| [0046](https://github.com/open1s/qwe/issues/46) | Low | README test counts inconsistent after badge refresh: badge 320 / suite-table total 304 / actual workspace 360; table prose says conformance 17/17 vs badge 18/18 | README.md:9, README.md:237 |
| [0047](https://github.com/open1s/qwe/issues/47) | Medium | Playground control endpoints accept cross-origin requests (no Origin/Host check + `ACAO:*`): any website can compile/run source, reset and pause; frames readable via `/state` | reference/src/present.rs (handle_playground) |
| [0048](https://github.com/open1s/qwe/issues/48) | Low | `/api/pause?on=0` can never resume - query string stripped before parsing, so `contains("on=0")` is dead code and the endpoint only ever pauses | reference/src/present.rs (handle_playground) |
| [0049](https://github.com/open1s/qwe/issues/49) | Low | Playground silently freezes on runtime step errors - driver pauses and discards the diagnostic, error pane stays empty | reference/src/present.rs (playground_driver) |
| [0050](https://github.com/open1s/qwe/issues/50) | Medium | `pwe fmt` is not string-aware: mutates multi-line string tokens (trim/collapse inside quotes) and counts braces/`#` inside strings for depth, so `format_preserves_tokens` is false for legal sources | reference/src/lang/format.rs |
| [0051](https://github.com/open1s/qwe/issues/51) | Low | `pwe fmt src \| head` panics on Broken pipe (rc 101) via bare `print!`, violating no-panic-in-runtime-paths | cli/src/main.rs (cmd_fmt) |
| [0052](https://github.com/open1s/qwe/issues/52) | Low | usage()/--help omits `playground` and `fmt`; both commit messages claim "usage updated" | cli/src/main.rs:53 |
| [0053](https://github.com/open1s/qwe/issues/53) | Low | repl: runtime step errors rendered via the compile-diagnose API as "error 18: unspecified compile error" (no step number/location); failed `:run` discards program + partial state; always exits 0; malformed `:run/:step N` silently default to 60/1 | cli/src/main.rs (run_repl / repl_steps) |
| [0054](https://github.com/open1s/qwe/issues/54) | Medium | assignment to an undeclared slot silently dropped during lowering (unknown-LHS skip with no diagnostic) while reads of unknown identifiers warn 85 - typo writes vanish with zero feedback | reference/src/lang/systems.rs:104-143 |
| [0055](https://github.com/open1s/qwe/issues/55) | Medium | playground CSRF guard bypass: `starts_with` Origin matching accepts `localhost.evil.com`/`127.0.0.1.evil.com`; Origin-less GET mutations (`/api/reset`, `/api/pause`) still execute | reference/src/present.rs (handle_playground) |
| [0056](https://github.com/open1s/qwe/issues/56) | Low | pwe fmt not comment-aware: a `"` inside a `#` comment opens a fake multi-line string (rest of file emitted verbatim, indentation stops); `//`-comment braces corrupt depth tracking | reference/src/lang/format.rs |
| [0057](https://github.com/open1s/qwe/issues/57) | Low | pwe doctest false-pass: warning-only blocks report success (check_document only records Err, never surfaces push_diag warnings); unclosed fence at EOF and outer 4-backtick fences silently check 0 blocks yet exit 0 | reference/src/doctest.rs (check_document / fenced_blocks) |

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

WGSL-backend review batch (0037–0038, commit `aee7ae30`):

1. 0037 — mirror of 0034 for the device backend: the emitted `Select`
   condition and the CPU oracle both use IEEE `!= 0.0f`, so `-0.0`
   flips the branch versus the interpreter's bit-test; the oracle ↔
   interpreter test grid never contains `-0.0`, so it stays green.
2. 0038 — the emitter is never executed or parsed: WGSL `sign(±0)`
   (spec: 0) ≠ Rust `signum` (±1), WGSL `round` is ties-to-even while
   Rust/interpreter are half-away-from-zero, the `hypot` `sqrt(a*a)`
   form overflows f32 at 1e20 where the oracle does not, and the
   `trunc`-based `Rem` emulation diverges from `fmodf` on 1.85M/4.1M
   sampled pairs (up to inf-vs-finite). The doc's "`naga` validation
   is added as a dev-dependency" claim is false — no `naga`/`wgpu`
   anywhere in the workspace; `validate_shader` is a bracket check.
3. Scope note added to 0036: `eligible()` also admits kernels whose
   interpreter run always traps (e.g. `a / 0.0`) while a device kernel
   would return inf/NaN — trap semantics need one decision for all
   backends. Both new backends remain un-wired (zero callers outside
   their modules), consistent with the RFC-0027 posture.

Fix-verification pass 4 (`e9387274`, pass 10 — verification/closure of 0036):

1. 0036 verified fixed and closed: `PweCtx.trap` shim records
   `EirInvalid 18` (first trap wins via `get_or_insert`), an
   `isnan(a) || isnan(b)` guard precedes every emitted compare, and
   the differential matrix gained `a < b` / `a == b` over a NaN input
   grid. Probe matrix (fresh process per mode): `lt/eq` with any NaN
   operand now `Err d18` on both tiers; non-NaN compares keep their
   previous results; `if <NaN flag>` CondBr traps on both; #30–#34
   probes re-run with no regressions (arity guards, div trap, sign,
   Select/CondBr bit-test). Gates: fmt, clippy, 317 tests,
   conformance 18/18.
2. TMPDIR audit: current tree leaks zero `pwe_native_*` dirs (probe
   process, filtered native tests, and a fresh full `cargo test
   --workspace` each leave the count unchanged); 7 dead-pid leftovers
   from pre-push dev runs were removed manually.

Fix-verification pass 5 (`20ba90be`, pass 11 — closures of 0037/0038,
filing of 0039):

1. 0037 verified fixed, closed: `Select` emits
   `bitcast<u32>(r[c]) != 0u` (no `!= 0.0f` left in the module) and the
   oracle mirrors it with `to_bits() != 0`; probe grid {0, -0.0, 1,
   -2.5, NaN} — interp == oracle == shader, `-0.0` now 11 (was 22).
2. 0038 verified fixed, closed: `naga` 30 (`wgsl-in`) is a real
   dev-dependency and every emitted shader in the matrix is parsed +
   validated; `sign`/`round`/`hypot`/`Rem` now emit `pwe_sign`,
   `pwe_round`, `pwe_hypot`, and float `%` — independent probes match
   CPU f32 references (bits for sign/round, tolerance for hypot/rem)
   and the interpreter on the original divergence cases (sign ±0,
   round ties, hypot 1e20/3.4e38, Rem trunc-form samples).
3. 0039 filed (Low): the remaining scope note — the device backend
   never traps (`1 % 0` → NaN, NaN compare → IEEE) while interp and
   native both `Err d18`; the module doc table documents it and the
   test matrix skips those rows pointing at an umbrella that did not
   exist until now.
4. Gates at `20ba90be`: fmt, clippy, 317 tests (wgsl/naga live),
   conformance 18/18.

Review/test pass on `70ca8d7f` (phase-3 hotness→native JIT, pass 12):

1. Gates on a pristine snapshot of the commit (the checkout carried
   unrelated uncommitted WIP): fmt OK, clippy clean, 318 tests (incl.
   `native_jit_promotes_and_matches_interpreter`), conformance 18/18.
2. RFC-0027 addendum present and matches the code: promotion runs only
   after `ready()` (Validate/CapabilityCheck/Publish), `-ffp-contract=off`
   guards FMA contraction, compile failures cache as deopt.
3. Library probes (`enable_native_jit(true)`, 1500 steps, ON vs OFF,
   step-jit and step-cross axes): poly / nbody / reactor promote
   (`native_executions>0`) and stay byte-identical; random/emit deopt
   (`native_executions=0`) and stay identical; trap kernel exposed #40.
4. CLI e2e on built fixtures: `pwe run` 1100 steps (past the ~1024-step
   CLI promotion point), 300000 steps, walker (random), and a
   PATH-stripped no-`cc` run — every `--native-jit` output
   byte-identical to plain, rc=0; promotion observed live (native build
   dir appeared mid-run). Production CLI is masked from #40 by
   `step_cross`'s interpreter-first ordering on clones.
5. 0040 filed (Medium): the deopt fallback re-executes the step after a
   failed native run already committed partial writes through
   `write_field` — trap disappears, statements double-apply
   (step 101: `w 0→-2`, `x 7→-7`, no error) — silent corruption via the
   public `step_jit` path.

Fix-verification pass 6 (`c7e7e3bd`, pass 13 — closure of 0039):

1. 0039 verified fixed, closed: emitted shaders declare an
   `atomic<u32>` trap binding (also required by `validate_shader`);
   `Div`/`Rem` guard `== 0.0` (±0) with flag-set + early return;
   compares use the naga-safe NaN idiom and trap the same way; the CPU
   oracle returns `EirInvalid 18` for those inputs. Probe parity:
   div0/rem0/NaN-compare all `Err d18` on both tiers, non-trap edges
   byte-equal, `Select`/`sign` do not over-trap (no regressions).
   Gates: fmt, clippy, 318 tests (naga live), conformance 18/18.

Review/test pass 14 (`b57ab005` + `93f69931` + `5f0429b9`, closure of 0040):

1. 0040 verified fixed, closed: the trap-after-write repro now behaves
   identically with native JIT ON and OFF (step 101 `Err d18`,
   `w=0 x=7`, native_exec=37, cross axis too) — was a swallowed trap
   with double-applied writes. `native_for`'s all-entries filter also
   deopts before mutation on a mixed-eligibility module (probe:
   `native_exec=0`, ON≡OFF); side finding: nondeterministic modules
   already failed `validate(true)` up front, so that path was
   defense-in-depth — the corruption was the `?`-propagation gap.
2. `b57ab005` gpu-verify crate independently executed on this machine:
   Apple M2 Max Metal adapter asserted, benign 64-lane kernel matches
   the CPU oracle, div-by-zero sets the device trap flag — first real
   device execution of emitted shaders; sticky-trap entry read keeps
   the trap binding alive under wgpu auto-layout. `5f0429b9` gitignore
   chore correct (`gpu-verify/target` untracked, sources tracked).
3. Gates on the tree: fmt, clippy, 319 tests (incl. the new #40
   regression test), conformance 18/18; full probe battery green
   (53 WGSL/edge PASS, t39 22/22, nrem/wcmp parity, jit-all 5 kernels).

Review/test pass 15 (`48492885` — native JIT on by default for `pwe run`):

1. Scope: `cmd_run` flips `native_jit = true` with a new `--no-native-jit`
   opt-out (`--native-jit` kept); library default stays opt-in, so tests
   and embedders are unaffected. `present` shares `cmd_run`, so it also
   defaults on (commit message mentions only `run` — noted, not a defect).
2. Gates on the tree: fmt, clippy, 359 tests green, conformance 18/18.
3. CLI e2e: 5000-step default run output byte-identical to both
   `--no-native-jit` and `--native-jit`; 300000-step default run
   byte-identical to interpreter run; promotion in the default path
   observed live (`pwe_native_*` dir at t=200ms) and cleaned up at exit
   (`Drop`); PATH-stripped no-`cc` default run rc=0, identical to
   interpreter (graceful deopt).
4. Observation (cosmetic, pre-existing pattern): `usage()` documents none
   of the run options (`--steps`/`--param`/`--native-jit`/`--no-native-jit`);
   left as a note rather than an issue.
5. Uncommitted WIP observed, untouched: `reference/Cargo.toml` gains an
   opt-in `gpu` feature (target-gated `metal` 0.33, `pwe run --gpu`).

Review/test pass 16 (`55457ff1` — Metal GPU backend for field sweeps):

1. Architecture verdict: elegant. Layering is textbook — `gpu` feature
   forwards cli → reference, `metal` is optional + `cfg(target_os)`
   gated (non-macOS/CI never builds it), `#[cfg(all(feature, macos))]`
   module and stub `enable_gpu` keep one CLI call site; the dispatch
   seam is the single `field_diffuse` domain op (no EIR bypass — the op
   still flows through the same IR path, only its implementation
   swaps), mirroring the `enable_native_jit` attach pattern; **both**
   `step_cross` runtimes attach the context, so the tier guard compares
   GPU-vs-GPU and cannot false-mismatch; `unsafe` post-wait readback is
   documented and isolated; roadmap/docs state the f32-approximate,
   CPU-is-oracle contract honestly.
2. Gates: fmt, clippy `--all-features`, 359 default tests,
   `--features gpu` reference suite 346 tests incl. 4 gpu tests on the
   Apple M2 Max, cli tests, conformance 18/18.
3. Behavior: release build without the feature → `--gpu` rc=1 (message
   quality filed as 0042); with the feature → banner + rc=0 for
   1/500/5000-step runs, no cross-guard failures. Field totals CPU vs
   GPU: rel Δ 9e-7 (500 steps), 1e-5 (5000), 4e-8 (16.8M cells/50) —
   bounded f32 drift, no blow-up.
4. Performance: the backend is net-slower everywhere tested (2.55× /
   2.0× / 1.34× at 0.26M / 2.1M / 16.8M cells) because `Gpu::diffuse`
   allocates, converts the whole field f64↔f32 and readbacks
   synchronously per step — filed as 0041 (Medium) with fix directions
   (device-resident field state first).
5. Style notes (not issues): the `cfg(all(feature, macos))` guard is
   repeated ~10× with 4 duplicated constructor lines in runtime.rs — a
   single `scene_rt` helper would collapse them; the commit message
   repeats the already-landed native-JIT-default line.

Fix-verification pass 7 (`81a46dd3`, pass 17 — closure of 0042):

1. 0042 verified fixed, closed: stub `enable_gpu` now returns detail 97
   ("GPU backend not built (rebuild with --features gpu on macOS)",
   covering the non-macOS case too) and `Gpu::new` failures return
   detail 98 ("no Metal GPU device available"); both names registered in
   `detail_name`, no collisions. Observed live: no-feature build rc=1
   with the new message; feature build banner + rc=0.
2. Residuals noted in the close comment (not filed): `Gpu::new` doc
   still says detail 5; MSL compile failure conflated with no-device in
   d98 (practically unreachable for the fixed inline kernel).
3. Gates: fmt, clippy `--all-features`, 359 tests, 346 tests
   `--features gpu`, conformance 18/18. 0041 (GPU path net-slower)
   remains open, untouched by this commit.

Fix-verification pass 8 (`03170cc5`, pass 18 — closure of 0041, nit filed as 0043):

1. 0041 resolved via offered fix direction 3 (relabel + benchmark record),
   closed: experimental-offload labels in `reference/Cargo.toml`, CLI banner
   and roadmap (ratios 1.34x-2.55x match the issue's measurements); the new
   `benches/throughput.rs::bench_gpu` verified live on Apple M2 Max at
   131.089us (CPU) vs 233.617us (GPU) — matches the commit's 129/244us claim;
   device-buffer pooling landed (my e2e 262k-cell/500-step GPU wall 0.51s ->
   0.33s vs CPU 0.199s, field totals unchanged). Device-resident field state
   (direction 1) documented in the roadmap as not implemented.
2. 0043 filed (Low): `Gpu::new` doc comment still says "Errors (detail 5)"
   — real details are now 98 (no device) / 99 (kernel compile); first noted
   in the 0042 close comment, still unfixed in this commit.
3. Gates: fmt, clippy `--all-features`, 359 tests, 346 tests
   `--features gpu`, conformance 18/18. Open issues after this pass: 0043.
4. Follow-up docs commit `e428e2dd` (run-flag help + stale roadmap note)
   verified: every documented flag exists in `cmd_run`, defaults match
   (60 steps, native JIT on, `--check` = detail 88 non-finite state), the
   roadmap #35 "not wired into `pwe run`" note now matches pass-15 reality;
   gates re-run green on `e3e21472`.

Fix-verification pass 9 (`381e3553`, pass 19 — closure of 0043):

1. 0043 verified fixed, closed: `Gpu::new` doc now cites detail 98 (no
   Metal device) / 99 (kernel compile), matching the code.
2. Gates: fmt, clippy `--all-features`, 359 tests, 346 tests
   `--features gpu`, conformance 18/18. Open issues: none.

Review pass 10 (`ecda9480` threaded dispatch + `0f0cd094` phase-4 docs,
filed 0044-0046):

1. `ecda9480` opt-in threaded-dispatch interpreter reviewed in depth: handler
   table mirrors `run_call_tree` (shared arith/divrem/compare helpers,
   identical Pow/Fma/unary/Select/branch/call/return/trap semantics), env
   untouched by any opcode in the supported subset, whole-module gating with
   fallback, table sized exactly to Unreachable=0x8004 (no OOB). My own
   bit-exact differential: 9 models x 300 steps (states + write-lists + trap
   errors) all identical, fallback verified with a random() model; CLI e2e
   byte-identical across default/--threaded x jit/no-jit (k.pweb,
   field3d.pweb). Bench claim verified: 2279.7us vs 3075.4us (~36% slower,
   stays opt-in). Gates: fmt, clippy -D warnings, 360 tests, conformance 18/18.
2. 0044 filed (Medium): `step_cross` never consults `self.threaded`
   (runtime.rs:584) yet commit message, test comment, roadmap and ADR-0003
   claim threaded-vs-JIT cross-verification; `pwe run --threaded` state is
   therefore never runtime cross-checked (comment added to the issue listing
   all four claim sites).
3. 0045 filed (Low): strategy divergence on error paths - th_get always
   detail 16 where the jump table uses 17 for operand 1; h_call reports
   byte_offset 0 where the jump table reports pc (detail 33 depth-cap is
   reachable on valid modules).
4. `0f0cd094` reviewed: architecture.md, CONTRIBUTING.md and ADR-0001/0002/0004/0005
   check out against measured facts (gates match CI incl. cargo-deny job,
   clippy.toml enforces the unwrap rule, ADR-0005 numbers match my #41
   measurements); ADR-0003 repeats the 0044 claim (see comment) and an
   unsubstantiated "micro ~8%" figure.
5. 0046 filed (Low): README counts internally inconsistent - badge tests-320
   vs suite-table total 304 vs workspace 360 measured at `0f0cd094`;
   table prose still says conformance 17/17 vs badge 18/18.

Review pass 11 (`8c3ed183` playground, filed 0047-0049):

1. Gates on a clean snapshot of `8c3ed183`: fmt, clippy `-D warnings`,
   361 tests (incl. `playground_page_has_editor_and_viewer`), conformance
   18/18. (An fmt failure first seen in the live checkout was dev's
   uncommitted WIP `format.rs` - Phase-4 `pwe fmt` - not a commit defect.)
2. Functional e2e live: editor `/` + `/view` + `/state` 200, sample compiles
   ("ok"), bad source renders a diagnostic (error 60), frames animate after
   Load, `/api/reset` ok, 127.0.0.1-only bind, `vendor_file` is an embedded
   whitelist (no traversal), error pane uses `textContent` (no XSS), driver
   publishes frames under the write lock, no unwrap/panic on runtime paths.
3. 0047 filed (Medium, security): no Origin/Host validation + `ACAO:*` on
   mutating endpoints - proven live with `Origin: https://evil.example`:
   POST /api/source answered 200 + ACAO:* (cross-origin compile+execute at
   ~62 steps/s), GET /api/reset 200. Impact = CSRF into the PWE sandbox
   (CPU DoS, state hijack, frame exfiltration); no OS-level access.
4. 0048 filed (Low): `/api/pause?on=0` - query stripped before parsing, so
   resume is dead code (live: returns "paused"); shipped UI never calls it,
   which is why manual e2e missed it.
5. 0049 filed (Low): runtime step errors pause the driver and drop the
   diagnostic (`take_diagnostics()` discarded) - viewer freezes with an empty
   error pane while compile errors do render.
6. Notes (not filed): `pwe --help`/usage() omits the new `playground`
   subcommand although the commit message says "usage updated" (same class as
   the earlier usage gap, fixed in `e428e2dd` for run flags); a `Load` whose
   compile exceeds the handler's 10 s `recv_timeout` still lands later from
   the driver queue after the client saw "compile timed out".

Review pass 12 (`a10b32f2` `pwe fmt`, filed 0050-0052):

1. Snapshot `git archive a10b32f2`; full gates green: `cargo fmt --check`,
   clippy `-D warnings`, 364 tests (+3 fmt), conformance 18/18.
2. CLI semantics verified: `--check` rc 0 formatted / rc 1 messy; `-w` then
   `--check` rc 0; `pwe run` output identical pre/post-format on the dev's
   fixtures; EIR-identity test passes but its fixture contains no strings.
3. 0050 filed (Medium): `format_source` is not string-aware. Multi-line
   strings are legal (`string = @\{ ... (!\"" ~ ANY)* ... \}`, ANY includes
   \n) but every line is trimmed and blank lines dropped, so
   `title = 'A   \n\n\n      B'` becomes `'A\n\n    B'` - token value
   changed, contract in module docs and `format_preserves_tokens` broken.
   Braces/`#` inside strings are also counted for depth (`title = "}"` emits
   the following block at indent 0). Fix: scan with in-string state; never
   trim/drop inside a literal.
4. 0051 filed (Low): `pwe fmt big.pwe | head -1` panics rc 101
   "failed printing to stdout: Broken pipe" (stdio.rs) - AGENTS §13.
5. 0052 filed (Low): usage() lists only compile/run/present/migrate; the
   `fmt` and `playground` commits both claim "usage updated" (false twice).

Review pass 13 (`42d7da70` `pwe repl`, filed 0053-0054):

1. Snapshot gates green: `cargo fmt --check`, clippy `-D warnings`, 366 tests
   (+2 repl), conformance 18/18.
2. Core semantics verified by differential against `pwe run` on the
   `na_water_demo` source (extracted from `reference/examples`): `:run 60`
   matches `pwe run --steps 60` (sim time, all positions); `:run 40` + `:step 20`
   equals `:run 60`; `:reset` + `:run 60` reproduces the fresh run exactly
   (`reset_to` restores scene/clock/env correctly).
3. 0053 filed (Low): three scriptable-core defects - (a) runtime step errors go
   through `lang::diagnose` (the compile API): output `error 18: unspecified
   compile error` with no step number/location vs `pwe run`'s `step 2 failed:
   PWE EirInvalid (18)` + diag lines; `take_diagnostics` is empty because
   runtime errors use `error()` not `error_at()`; (b) failed `:run` never stores
   the runtime, so `:step` afterwards claims `(no program yet...)` and partial
   state is lost; (c) exit code always 0 (tests hardcode it) and `:run abc` /
   `:run -5` silently run 60 steps (`cmd_run --steps abc` correctly exits 2).
   Comment added to 0052 (third false "usage updated" - `usage()` still has no
   `repl` line; `cmd_repl` ignores argv).
4. 0054 filed (Medium): `systems.rs` rule/assign lowering skips unresolved LHS
   names with no diagnostic - `y = 5.0` on an entity whose state has only `x`
   compiles clean and the write vanishes (`state=[3.0000]` after `x=x+1` runs
   3 steps), while reads of the same unknown name emit warning 85 whose own
   comment says "a typo must not silently do nothing". Comment added to 0050
   (roadmap repeats the disproved fmt guarantee; repl typed input has the same
   trim/blank-drop string defect - `:load` is exact).

Review pass 14 (`d03bb9ae` fix batch #44-#52, filed 0055-0056):

1. Snapshot gates green: fmt, clippy `-D warnings`, 367 tests, conformance
   18/18. Commit adds exactly one test (fmt string-awareness); #44/#45/#47/#48/#49
   ship without regression tests.
2. Verified & closed: 44 (step_cross gate now identical to step_interpreter at
   runtime.rs:517/585; 500-step na_water default/threaded x jit/nojit all
   byte-identical, trap program identical rc 1; thd probe 9/9), 46 (README
   badge/table 367 & 18/18 match actual), 47 (live: ACAO gone, evil Origin 403;
   residuals -> 0055), 48 (query parsed pre-strip, on=0 resumes, live ok),
   49 (live: trap -> `info: ['step 2 failed: PWE EirInvalid (18) at 0']`,
   good program resumes), 50 (multi-line title preserved verbatim
   `'A   \n\n\n      B'`, `}`-string depth correct), 51 (`fmt | head` rc 0,
   no panic), 52 (usage lists playground/repl/fmt).
3. 45 kept open (comment): operand details fixed & match jump table (incl.
   Select false-branch 17, WriteView 25), but call errors 32/33/34 still
   byte_offset `pc` (eir.rs:1278-1291) vs `0` (:3016-3028).
4. 0055 filed (Medium): CSRF guard uses `starts_with` - live bypass
   `Origin: http://localhost.evil.com` -> 200 (as does 127.0.0.1.evil.com);
   and Origin-less GET `/api/reset` -> 200 (browsers omit Origin on no-cors
   GETs; both mutation endpoints are GET). Fix: exact host parse + POST-only
   mutations / Sec-Fetch-Site.
5. 0056 filed (Low): fmt still not comment-aware - quote inside `#` comment
   opens fake string (rest of file verbatim, indent stops), `//`-comment
   braces decrement depth (cascading mis-indent). Both verified semantics-safe
   (run output identical, idempotent) - formatting correctness only.
6. Still open, untouched as claimed: 53 (repl), 54 (silent unknown-LHS).

Review pass 15 (`83ce2e63` fix #53/#54, `3690528c` fix #55/#56):

1. Snapshot gates green for both fix commits: fmt, clippy `-D warnings`,
   conformance 18/18; tests 370 (after `83ce2e63`) -> 373 (after `3690528c`
   + docs `33b8d1ef`), matching the README badge/table refresh.
2. 54 verified & closed (`83ce2e63`): warning detail 100 on unknown named LHS
   in UpdateSystem rules/assigns + Rk4System rules; negative control fires,
   3 duplicate bad writes dedupe to 1; false-positive sweep clean - all 32
   repo `.pwe` files compile with zero 100/85, incl. `structs.pwe` and the
   exact doc struct dotted-LHS example (dotted `state_names` resolve);
   `s[expr]` assigns partition to dyn_assigns first; read-85 regression intact;
   `law.pwe` failure pre-existing (identical error 62 on base `a0dfeedd`).
3. 55 verified & closed (`3690528c`): live - no token 403, token 200, both
   evil Origins 403 (was 200), local Origin 200, Sec-Fetch-Site cross-site 403,
   wrong token 403, `/reset`+`/pause` aliases 403 unguarded (previously open),
   POST `/api/source`+token 200, OPTIONS preflight 403 with no
   `Access-Control-*`. Mandatory token also covers Origin-less/SFS-less legacy
   requests (cross-site pages cannot read or set the header).
4. 56 verified & closed (`3690528c`): quote-in-`#`-comment and braces-in-`//`
   counterexamples fixed; `#`/`//` inside real multi-line strings and
   `"http://"` in strings intact; idempotent 32/32; original vs formatted
   compile diagnostics identical and 10-step run output byte-identical
   (31/32 run-compared, `law.pwe` error 62 on both sides).
5. 53 kept open (comment): fixed - exit codes (error rc 1, sticky), malformed
   counts rejected, failed `:run` keeps program, real runtime error detail,
   2 tests. Remaining - no step number in `step failed:` (53a), `:load`
   failure and empty `:run` still exit 0, `:run 5 extra` ignores extras.

Review pass 16 (`9c1167d6` `pwe doctest`, filed 0057):

1. Snapshot gates green: fmt, clippy `-D warnings`, 377 tests (373 + 4
   doctest), conformance 18/18, release build ok.
2. Feature verified live: default run checks `docs/lang-usage.md` (13 runnable
   blocks, rc 0); all 4 shipped docs 28 blocks rc 0; bad block -> `{path}:{line}:
   code block does not compile` + full diagnostic, rc 1; `pwe text`/fragment/
   `pwe ignore` blocks correctly skipped; missing file -> message + rc 1;
   usage lists `pwe doctest [FILES...]`.
3. Skip heuristic audited: the 5 non-runnable `pwe` fences in lang-usage are
   genuine fragments (entity/funcs/params-only) or file-illustration snippets
   that do not compile standalone (verified by extracting them) - the
   `starts_with("world")` rule under-checks nothing that compiles today.
4. 0057 filed (Low): two false-pass classes - (a) warning-only blocks report
   success: `x = x + ghost + 1.0` block passes doctest rc 0 while compile warns
   85 (check_document only reads Err; shipped_docs_compile inherits the gap);
   (b) unclosed fence at EOF drops the last block -> "0 runnable block(s)",
   rc 0; outer 4-backtick fences likewise silently check nothing.
5. Diagnostic hygiene checked: a warn-block followed by a fail-block reports
   only the failure's own error (no stale cross-block diagnostics).

Local copies of the bodies live next to this file (`0001-…` … `0035-…`).
