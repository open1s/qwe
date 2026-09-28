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

Local copies of the bodies live next to this file (`0001-…` … `0035-…`).
