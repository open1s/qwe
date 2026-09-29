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
| [0058](https://github.com/open1s/qwe/issues/58) | Medium | pwe lsp publishes a bogus parse failure (code 60, range 0:0) for any document with `import` - in-memory `lang::compile` = `parse+compile_program` has no module loader while CLI `compile_file` uses `load_program_sources`; same root cause makes doctest unable to check import doc blocks | cli/src/lsp.rs (diagnostics), reference/src/lang/compile.rs:1908 |
| [0059](https://github.com/open1s/qwe/issues/59) | Low | pwe lsp diagnostic ranges are 0:0 for warnings (85/100/94) and parse errors: Ok-path hardcodes `diag_json(..., 0, ...)` ignoring `d.byte_offset`, and upstream push_diag sites record 0 (error_at errors like 52 map correctly - positive control) | cli/src/lsp.rs (diagnostics), reference/src/lang/diagnostics.rs |
| [0060](https://github.com/open1s/qwe/issues/60) | Low | pwe lsp/json non-BMP handling: `\uD800-\uDFFF` surrogate escapes decode independently to U+FFFD (document text corrupted for escaping clients); `pos_at` counts code points, not UTF-16 units (astral columns off; BMP/CJK unaffected) | cli/src/json.rs (string), cli/src/lsp.rs (pos_at) |
| [0061](https://github.com/open1s/qwe/issues/61) | Low | fuzz ALPHABET has no digits/`_`/`@`/`~`/`^` - numeric-literal, slot-index and unit-annotation paths never fuzzed (0% of samples contain a digit); single hard-coded seed, 160-byte cap | reference/tests/fuzz.rs (parser_and_compiler_never_panic_on_random_source) |
| [0062](https://github.com/open1s/qwe/issues/62) | Medium | import documents compile from DISK, not the client buffer: any text containing substring `import` routes didOpen/didChange through `load_program_sources(path)` -> buffer errors vanish, phantom disk errors appear | cli/src/lsp.rs (diagnostics, `text.contains("import")`) |
| [0063](https://github.com/open1s/qwe/issues/63) | Low | warning-range heuristic `backtick_name` + `text.find(name)` matches the first substring anywhere: unknown slot `x` ranges to `x` inside `xyzzy` in a comment on another line | cli/src/lsp.rs (diag_json, backtick_name) |
| [0064](https://github.com/open1s/qwe/issues/64) | Low | docs/rfc-alignment.md extension table stops at 0044 (despite carrying Proposed rows 0043/0044): filed Proposed RFC-0045/0046 missing while roadmap + book SUMMARY index them | docs/rfc-alignment.md (Extension RFCs table) |
| [0065](https://github.com/open1s/qwe/issues/65) | Medium | `module` directive stripped only by `collect_module`: in-memory paths (`lang::compile`, LSP `with_root` buffer override) report bogus error 60 on valid module files; doctest skips module blocks; `merge_sources` skips the 101 dup check | reference/src/lang/compile.rs (collect_module vs lang::compile/load_program_sources_with_root) |
| [0066](https://github.com/open1s/qwe/issues/66) | Low | module-line stripping is string-blind: a multi-line string containing a `module x` line silently loses it (compile succeeds, title/content corrupted) | reference/src/lang/compile.rs (collect_module strip loop) |
| [0067](https://github.com/open1s/qwe/issues/67) | Medium | detail 102 privacy check walks only `parsed.systems`: `util.g` (unexported) compiles from a `funcs` body; non-`when` system params not walked either | reference/src/lang/compile.rs (check_module_privacy) |
| [0068](https://github.com/open1s/qwe/issues/68) | Low | `strip_directives` in-string tracker (naive odd-quote count) is poisoned by an unbalanced `"` in a comment: later directive lines not stripped → bogus error 60 (CLI regression vs always-strip) | reference/src/lang/compile.rs (strip_directives) |
| [0069](https://github.com/open1s/qwe/issues/69) | Low | LSP buffer override runs after the 101 duplicate-module check and never re-checks: buffer-declared `module` collision passes LSP (EMPTY diagnostics) while CLI rejects with 101 | reference/src/lang/compile.rs (load_program_sources_inner order) |
| [0070](https://github.com/open1s/qwe/issues/70) | Medium | `from … import` alias resolution indexes `modules` with a stale `seen` map after the RFC-0045 reorder: an unexported `from … import g` compiles (detail 102 bypassed) and silently binds to another module's `g` (probe `state=[101.0]` = zmod's `g`, not amod's) | reference/src/lang/compile.rs (load_program_sources_inner :1466-1474 reorder, :1494-1499 lookup) |
| [0071](https://github.com/open1s/qwe/issues/71) | Medium | `merge_sources` re-derives module declarations from raw text (not the string-aware stripper): an in-string `module` line compiles fine but the artifact fails to run with a false `error 101` — compile/run divergence | reference/src/lang/compile.rs (merge_sources `lines().find_map(parse_module_line)`) |
| [0072](https://github.com/open1s/qwe/issues/72) | Medium | RFC-0046 body ≠ shipped impl: the Status line was rewritten for the `block_count>1`-in-FUNCTIONS form, but Design/Wire-format still prescribe a BLOCKS section / FUNCTIONS minor bump and a `Block{params,ops,term}` model; RFC-0021 (frozen v0.2.0) not updated and `EIR_MINOR` stays 0 — same program, different bytes, same declared version | rfc/RFC-0046-eir-ssa-cfg.md (Design / Wire format), rfc/RFC-0021 §Functions, reference/src/eir.rs :41-42, :2100-2112 |
| [0073](https://github.com/open1s/qwe/issues/73) | Low | `encode_functions_section` now routes artifacts through `blocks_from_instructions` (write path); that helper panics on an empty instruction stream (`e-1` underflow) or an out-of-range `Br`/`CondBr` target (`instructions[e-1]` OOB) | reference/src/eir.rs (encode_functions_section :2077/:2090, blocks_from_instructions :2229) |
| [0074](https://github.com/open1s/qwe/issues/74) | Low | README test counts stale: badges + `cargo test` comment + table totals say 399 in both languages while the workspace is 402 (f38aac54 +2, f4726072 +1, neither refreshed); unit row 347 needs 350 | README.md:9/:223/:237-244, README-ZH.md:8/:210/:226/:232 |

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

Review pass 17 (`c5509c60` `pwe lsp`, filed 0058-0060):

1. Snapshot gates green: fmt, clippy `-D warnings`, 382 tests (377 + 5 json/lsp),
   conformance 18/18, release build ok; usage lists `pwe lsp`.
2. Live LSP session probed end-to-end: initialize (Full sync +
   documentFormattingProvider), didOpen error diag (code/message/severity ok),
   warning diag severity 2, didChange republish, didClose clears to [],
   formatting applies real indentation edits and is idempotent at the LSP level,
   single-line balanced block no longer inflates depth (format.rs signed-net
   fix; unit `single_line_block_keeps_depth`), malformed JSON body tolerated
   (server survives), unknown request with id -> null (never hangs),
   shutdown -> null, exit -> rc 0, EOF -> rc 0.
3. fmt regression re-run on this commit (format.rs changed): 32/32 repo .pwe
   idempotent, compile diagnostics + 10-step runtime byte-identical, both
   comment counterexamples (#56) still fixed.
4. 0058 filed (Medium): any document with `import` gets bogus error 60 at 1:1 -
   `lang::compile` (compile.rs:1908) parses without the module loader while the
   CLI's `compile_file` uses `load_program_sources`; every std-importing example
   would show a phantom parse error in the editor; also explains why doctest
   can never check import blocks.
5. 0059 filed (Low): diagnostic ranges 0:0 for warnings and parse errors -
   Ok-path passes literal 0 instead of `d.byte_offset`; upstream 85/100/94
   push_diag sites also record 0; `error_at` errors (52) map correctly (control).
6. 0060 filed (Low): non-BMP - surrogate-pair escapes decode to U+FFFD and
   `pos_at` counts code points instead of UTF-16 (astral-only; CJK fine).

Review pass 18 (`61e38bd6` fuzz + Miri, filed 0061):

1. Snapshot gates green: fmt, clippy `-D warnings`, 384 tests (README badge,
   prose and suite table all match actual; breakdown 334+18+4+3+2+23 verified),
   conformance 18/18.
2. Shipped fuzz verified: `cargo test -p pwe-reference --test fuzz` passes in
   0.10 s (deterministic seeds, CI-cheap); `cfg!(miri)` reduces iterations.
3. Documented Miri command verified verbatim: `rustup component add miri
   --toolchain nightly` then `cargo +nightly miri test -p pwe-reference
   --test fuzz` -> 2 passed, 0 failed (62.7 s).
4. Stress-hunted beyond the shipped suite (out-of-tree, not committed): 5 seeds
   x 20 000 formatter inputs with digits/`_`/`@`/`~` added + 4 x 50 000 decoder
   byte inputs + 2 x 20 000 compile inputs -> zero panics, zero idempotence
   failures. Invariants hold under heavier fuzzing.
5. 0061 filed (Low): shipped ALPHABET contains no digits (nor `_@~^`), so
   numeric literals, `sN` slot indices and unit annotations are structurally
   unreachable for the fuzzer; single fixed seed + 160-byte cap. Coverage gap
   only - the heavier digit-inclusive hunt found no product bug.

Review pass 19 (three commits at once — `26ebc5ee` serde_json refactor,
`e8ce691e` fix(#57), `004aadd1` fix(#58-#61); closed 0057-0061, filed 0062-0063):

1. Snapshot `004aadd1` (includes both earlier commits) gates green: fmt,
   clippy `-D warnings`, 387 tests (README claim verified), conformance 18/18;
   release `pwe` built for black-box probing.
2. 0057 (e8ce691e) verified live: warning-only block prints
   `file:line: warning: [85] ...` (rc 0; rc 1 with `--strict`); unclosed fence
   = failure + rc 1; 4-backtick pwe fence now checked (inner `nope` -> error 59,
   rc 1); shipped default sweep 13 blocks rc 0 clean; `--strict` not treated as
   a filename.
3. 0058 verified for `file://`: import doc at a resolvable path publishes
   empty diagnostics (was bogus 60). BUT found the fix compiles the disk file
   instead of the sent buffer (`contains("import")` gate) — buffer edits
   ignored, phantom disk errors on clean buffers; non-import control still
   buffer-based. Filed 0062 (Medium).
4. 0059 verified: ghost warning range now line 1 character 40 (was 0:0).
   Residual: `text.find(name)` matches any earlier substring — `x` located
   inside `xyzzy` in a comment on line 0. Filed 0063 (Low).
5. 0060 verified across both commits: `pos_at` counts UTF-16 units
   (`position_helpers`); hand-rolled JSON parser replaced by serde_json
   (26ebc5ee) — readback via formatting edit shows the real 😀 for a literal
   `😀` escape, no U+FFFD.
6. 0061 verified: alphabet gains digits/units/`sN`, 3 seeds, 400-byte cap;
   fuzz tests pass.
7. 26ebc5ee behavior parity re-probed end-to-end on the new build: initialize
   capabilities, didOpen/didChange diagnostics, formatting (edit + no-op),
   garbage JSON tolerated, unknown method -> null, shutdown -> null, exit 0 —
   all unchanged.
8. Issues closed this pass: 0057, 0058 (residuals noted), 0059 (residual
   filed), 0060, 0061. Open: 0045, 0053, 0062, 0063.

Review pass 20 (`dce1cbaa` RFC<->conformance for extension RFCs 0037-0042; no new issues):

1. Snapshot gates green: fmt, clippy `-D warnings`, 387 tests, release build.
2. `cargo run -p pwe-conformance` live: **total=23 failed=0** (18 prior + 5 new
   cases: 0037 field sweep, 0038 pool, 0039 joints, 0040 soft bodies, 0042
   struct records) — all interpreter==JIT cross-backend; report matches the
   README/ZH badges and prose (23/23).
3. Sensitivity check (mutation probe, out-of-tree): the RFC-0039 invariant
   discriminates — same chain with joint systems removed ends at
   d(anchor,b1)=19.232 (>3.0 -> would FAIL) vs 2.589 bounded with joints.
   Pool/soft count invariants are inherently discriminating; probe also
   confirmed 1-based EntityIds as the cases assume (`finite(&rt,5)` = bob).
4. Docs consistency: rfc-alignment 0029 row updated to total=23; new
   Extension RFCs (0037-0044) table present, 0043/0044 marked Proposed;
   0041 evidence exists (`present::tests`, vendored `/vendor/three/...`);
   roadmap P4.3 updated.
5. Nit (not an issue): commit message says "six" cases — the commit adds
   five (18+5=23); the sixth extension RFC (0041) is covered by
   `present::tests`, not this binary.

Review pass 21 (`e394054e` fix(#62,#63); both kept open, partial):

1. Snapshot gates green: fmt, clippy `-D warnings`, 387 tests, conformance
   23/23, release build.
2. 0062 partial: buffer tracking works — didChange injecting `nope_lerp` into
   an import doc publishes warning 85 at the buffer's line; valid buffer ->
   `[]` (`load_program_sources_with_root` parses buffer, imports from disk).
   BUT filed evidence case 3 still reproduces verbatim: disk = `BROKEN {`,
   clean buffer -> `error 60 @1:1` from disk (collect_module parses the disk
   root before the override runs), and a buffer that *adds* an import gets a
   false `math.lerp` error 59 (namespace still disk-derived). Commented on
   0062 with probes; issue stays open.
3. 0063 partial: the filed `xyzzy` case fixed — unknown slot `x` now ranges
   to line 1 char 32 (was line 0 inside `xyzzy`). Residual: first whole-token
   anywhere still matches comments (`# ghost` line 0 wins over code line 1),
   and the commit message claims `find_ident` tests that do not exist (zero
   `#[test]` in the diff). Commented on 0063; issue stays open.
4. Probe battery on the build: import A/B controls, untitled fallback (still
   parse 60, accepted per 0058 close), non-import buffer control — all as
   expected.

Review pass 22 (`8fec1ebc` book SUMMARY index for RFC-0041..0046; filed 0064):

1. Docs-only change: 6 new SUMMARY.md entries. All six target files verified
   to exist with exactly matching names (`rfc/RFC-0041-…` … `RFC-0046-…`);
   no broken book links. No code touched — no gates required.
2. Cross-index consistency sweep triggered by the commit: roadmap references
   0045/0046 (Proposed) ✓, RFC files exist with `Status: Proposed` ✓, but
   `docs/rfc-alignment.md`'s extension table (0037-0044) — which already
   carries unimplemented Proposed rows 0043/0044 — omits both. Filed 0064
   (Low).

Review pass 23 (`f765dcad` RFC-0045 slice: `module` directive + detail 101 +
deterministic merge order; filed 0065-0066):

1. Snapshot gates green: fmt, clippy `-D warnings`, **388** tests (387 + new
   `module_declaration_and_collision`), conformance 23/23, release build.
2. Verified working: CLI `compile_file` strips `module <name>` (rc 0) and the
   declared name resolves as a qualified alias (`util.f`) with no import;
   duplicate declared names in imported files → `error 101` with both paths
   (CLI rc 1 and through the LSP); detail 101 documented en+zh; module line
   replaced by a blank line (offsets preserved).
3. 0065 filed (Medium): the directive is stripped only in `collect_module`.
   In-memory paths disagree with the loader on the same source — LSP
   `didOpen` of a valid module file (with or without imports) publishes bogus
   `error 60 @1:1` (also masking real buffer errors); doctest reports `0
   runnable block(s)` for a module block (unchecked); `merge_sources` never
   re-runs the 101 check.
4. 0066 filed (Low): quote-free `module` lines inside multi-line strings are
   stripped pre-parse → `title = "…\nmodule util\n…"` compiles rc 0 with the
   line silently deleted (string corrupted). The import analogue cannot occur
   inside one string (quotes close it first).
5. Regression battery (pass-21 probes) re-run on this build: A/B, xyzzy
   position, non-import control unchanged; C phantom-60, comment-ghost,
   untitled-60, import-drift residuals still exactly as recorded on the open
   0062/0063/0058 notes — no behavioral change from this commit.

Review pass 24 (`582bed3a` RFC-0045 slice 2: `export` surface + privacy detail
102; filed 0067, commented 0064/0065/0066):

1. Snapshot gates green: fmt, clippy `-D warnings`, **389** tests (388 + new
   `module_export_privacy`), conformance 23/23, release build.
2. Verified working: `export f` and `export { f }` both parse; unexported
   `util.g` in a **system** rule → `error 102` (CLI); exported `util.f`
   compiles; detail 102 documented en+zh; privacy check runs after merge in
   the file-loader path.
3. 0067 filed (Medium): privacy walks only system expressions — root
   `funcs { h(x) { util.g(x) } }` compiles rc 0 (proven), so the documented
   "cross-module reference to an unexported function is rejected" claim has a
   funcs-body hole; non-`when` system params also unwalked.
4. 0065 commented: `export` lines hit the same in-memory strip gap (LSP bogus
   60 on module+export docs, with and without imports; CLI rc 0 on the same
   source), and `merge_sources` hard-codes `exports: None` so 102 never runs
   on the rebuild path.
5. 0066 commented: `export f` line inside a multi-line `title` silently
   stripped, compile rc 0 (same string-blind loop).
6. 0064 commented: 0045 row added but placed between 0043/0044, header still
   (0037-0044), 0046 row still missing, row says Done while the RFC says
   Accepted + Remaining items, and "Accepted" is off-convention (0037-0042
   implemented extensions all use `Status: Normative`).

Review pass 25 (`7aadc284` fix(#64-#67): shared string-aware directive
stripping; module privacy in funcs; closed 0065-0067, filed 0068-0069):

1. Snapshot gates green: fmt, clippy `-D warnings`, **392** tests (389 + 3 new
   module tests), conformance 23/23, release build.
2. 0065 closed: one string-aware `strip_directives` now serves
   `collect_module`, the `with_root` override, and `lang::compile` — LSP probes
   (module+export, module+import+export) → EMPTY diagnostics (both were bogus
   60 @1:1); CLI 101 fixtures intact; `merge_sources` runs the shared 101
   check; `is_runnable` accepts directive-first doctest blocks. Residuals
   recorded on the close comment (buffer import drift → 0062, 101-after-
   override ordering → 0069, `merge_sources` 102 gap, doctest import trap).
3. 0066 closed: behavioral probe — a `module util` line inside a multi-line
   `title` is not collected as a declaration (no false 101 against a real
   child `module util`) and compiles rc 0; unit test asserts preservation.
4. 0067 closed: funcs-body `util.g` now → error 102 rc 1 (was rc 0); LSP
   import-doc path reports 102; `send { value = util.g(x) }` → 102; state and
   world-params expressions holding calls are rejected by the grammar (no
   bypass host left).
5. 0068 filed (Low): `strip_directives` toggles string state via
   `matches('"').count() % 2` — a comment with an unbalanced quote poisons it,
   later `import` line not stripped → error 60 @2:1 (CLI regression: the old
   collector always stripped).
6. 0069 filed (Low): 101 check runs before the buffer override; buffer adding
   a colliding `module util` → LSP EMPTY while CLI rejects with 101.
7. 0062 updated (kept open): original case 3 still reproduces (disk `BROKEN {`
   → phantom 60); new evidence — buffer adds/removes an `import` line → false
   59 vs CLI rc 0; directive-parse half of the original report now fixed.
8. 0064 updated (kept open): 0046 row added, but header still says
   (0037-0044), rows read 0043/0045/0046/0044, status vocabulary still mixed
   (`Done`/`Accepted` vs repo-standard `Normative`).
9. Regression battery (`lsp_probe62` suite): A EMPTY, B surfaces buffer error,
   C/#63 residuals unchanged, untitled+import EMPTY, non-import control fine —
   no behavioral drift beyond the fixes.

Review pass 26 (`ddc88472` fix(#45,#53,#62,#63): finish the partial fixes;
issue triage; closed 0045/0062/0063/0069 as completed, 0053 as won't-fix):

1. Snapshot gates green: fmt, clippy `-D warnings`, **393** tests (392 + 1),
   conformance 23/23, release build.
2. 0045 closed (completed): threaded call errors (32/33/34) now carry `m.pc` —
   depth-cap program reports `EirInvalid (33) at 2` under **both** dispatchers
   (threaded said `at 0`); threaded per-operand details use
   `th_get_d(…, 16/17/18)` matching the jump-table arms.
3. 0062 closed (completed): `collect_module` accepts an in-memory root and
   skips the disk parse — disk `BROKEN {` + valid buffer → EMPTY (was phantom
   60); buffer-added import resolves → EMPTY (was false 59); buffer syntax
   errors surface at buffer positions; untitled+import still EMPTY.
4. 0063 closed (completed): `find_ident` masks comments/strings — original
   `xyzzy` evidence now reports line 1, char 32 (was line 0, char 55);
   comment-ghost range moved onto the code line.
5. 0069 closed (completed, side effect of the #62 refactor): LSP now reports
   error 101 for a buffer-declared module collision; LSP and CLI agree.
6. 0053 closed (won't-fix) after re-verification — main defects already fixed
   in the previous build (step-error rendering, retained program, exit 1,
   malformed-arg rejection); ddc88472 added step index + had_error coverage.
   Residual `cmd_repl()` ignoring argv (`pwe repl --typo` starts the REPL)
   deemed cosmetic — documented in the close comment.
7. Still open and re-confirmed: 0064 (alignment header/order/vocabulary
   untouched by this commit), 0068 (odd-quote comment poisons the stripper →
   CLI and LSP both still report bogus 60 at 2:1).
8. Regression battery green: A/C/untitled/controls EMPTY as expected;
   `funcs`-body 102, string-preserve, dup 101 all hold; buffer-REMOVES-import
   59 is correct content (usage kept, import dropped), not drift.

Review pass 27 (`132811a9` fix(#68): strip_directives ignores quotes inside
comments; closed 0068 as completed):

1. Snapshot gates green: fmt, clippy `-D warnings`, **394** tests (393 + 1),
   conformance 23/23, release build.
2. 0068 closed (completed): `scan_string_state` breaks at `#`/`//` outside
   strings — original odd-quote comment + `import` line → CLI rc 0 and LSP
   EMPTY (both were bogus 60 at 2:1); `//`-comment variant rc 0.
3. String-content edges verified: `#` and `//` inside multi-line strings do
   not end the string; a real `import` after such strings still strips
   (rc 0); `#66` preservation holds (no false 101).
4. No regressions: full battery (A/B/C, xyzzy, comment ghost, untitled,
   import tracking, control) byte-identical to pass 26; 101/102 still fire;
   `#69` LSP 101 still reported.
5. Probe design note: an apparent failure was two `world` sections in one
   fixture (legitimate parse error), not the stripper.

Only **0064** remains open (alignment header/order/vocabulary — untouched by
the last two commits).

Review pass 28 (triage: closed 0064 as completed):

1. Verified on current main (no new dev commits): both previously missing
   rows exist — 0045 (`Done`, test evidence) and 0046 (`Proposed`, accurate
   note) — so the issue's title gap is resolved.
2. Cosmetic residuals remain and were documented in the close comment as
   non-blocking routine docs polish: header still `(0037-0044)`, row order
   0043/0045/0046/0044, mixed status vocabulary (`Done`/`Accepted` vs
   sibling `Normative`).
3. **Zero open issues** as of this pass.

Review pass 29 (RFC-0045 from-import review; filed 0070):

1. Reviewed the uncommitted RFC-0045 module-system slice (working copy `@`
   `ff2f49ed`, parent `f741f570`): the `module`/`export`/`import`/`from … import`
   directives, the shared string-aware stripper, `check_module_privacy`, 2 new
   unit tests, and 1 conformance case (`modules`). Gates green at the copy:
   fmt, clippy, both new tests pass, `pwe-conformance` compiles.
2. 0070 filed (Medium): `load_program_sources_inner` reorders `modules`
   (root out → stable sort by `(ns, version, path)` → root back, :1466-1474)
   but the `from … import` alias loop still resolves the child namespace via
   the **pre-reorder** `seen` index (`:1494-1499` → `modules[i].ns`). With ≥2
   imports whose discovery order ≠ ns-sorted order the index points at a
   sibling, so detail 102 consults the wrong module's `export` surface and the
   alias is rewritten to the wrong qualified name. Probe (CLI `pwe compile`):
   the identical `from "amod" import g` (g unexported) gives `error 102` with
   `import "amod"` alone and with `amod`-then-`zmod`, but **compiles rc 0**
   with `zmod`-then-`amod`; `pwe run --steps 1` of the artifact yields
   `state=[101.0]` (zmod's `g` = x+100), not the imported `amod.g` (x−1) —
   a silent, order-dependent wrong binding. The existing 2-module tests can
   never trigger the reorder, so the defect is uncovered.
3. Noted (not filed): the export-surface gate runs only at compile time —
   `merge_sources` never calls `check_module_privacy` and `ProgramSources`
   drops `exports`. Acceptable today (artifacts come from already-validated
   compiles); would matter if artifacts are ever loaded from untrusted sources.

Review pass 30 (`23190a53` RFC-0045 complete + `9fdb4515` RFC-0046 basic
blocks + `162af962` fix(#70); verified 0070 fixed & closed, filed 0071):

1. Snapshot gates green at `162af962`: fmt, clippy `-D warnings`, **398**
   tests (394 + 4), conformance **25/25** (two new RFC-0045 cases: cross-
   backend modules + export surface). Builds of 9fdb4515 were green too
   (397/25).
2. 23190a53 verified: `from "u" import g` (unexported) → detail 102 on CLI
   and LSP (position at the item name); exported import compiles; `module
   util 1.0` parses and compiles; artifact hash changes when an imported
   module's comment changes (RFC-0045 identity claim, hash
   `baacf37f… → 98e5297a…`); README/README-ZH/alignment/RFC updated.
3. 9fdb4515 verified: old `.pweb` (ddc88472 build) runs on the new binary;
   new straight-line artifact runs on the old binary; branched-gate program
   runs under `--native-jit`; `eir_cfg_blocks_round_trip` covers block
   encode/decode determinism; 0046 row → Done, roadmap checkbox updated.
   Residual noted: RFC-0046 uses `Accepted` (off the sibling `Normative`
   convention) — cosmetic, non-blocking per the 0064 close.
4. 0070 independently verified on 9fdb4515 (bug case rc 0 + `state=[101.0]`
   wrong binding vs control 102) and **closed** after 162af962: both orders
   now give 102, and with both modules exporting `g` the artifact binds
   `amod.g` (`state=[0.0]`, order-independent). Comment with probes on the
   issue; regression test `from_import_privacy_survives_module_reorder`.
5. 0071 filed (Medium): `merge_sources` uses raw `lines().find_map(parse_module_line)`
   on embedded (post-strip) sources, so a `module util 9.9` line inside a
   multi-line `title` is *content* at compile time but a *declaration* at run
   time — `pwe compile` rc 0, `pwe run` → `error 101: duplicate module name
   'util' (in <root> and t/u2.pwe)`. Still reproduces on 162af962. Same fix
   (use `strip_directives`) would also restore the 101/102 checks on the run
   path (noted, not separately filed, in pass 29).
6. This file gained a parallel pass-29 record and a 0070 index row from the
   concurrent reviewer; both are kept — pass 30 above complements pass 29
   (WIP review vs landed review + fix verification).

Review pass 31 (spec-sync review of `9fdb4515` RFC-0046; filed 0072-0073):

1. Corrected the pass-30 note above. The wire gap is **not** "RFC-0021 lacks
   `block_count`" — RFC-0021 §Functions already carries `u32 block_count |
   blocks`. The real gaps are: (a) RFC-0046's normative body (Design /
   Wire format / Validation) still describes the *abandoned* BLOCKS-section /
   minor-bump design and a `Block{params,ops,term}` + `Terminator` model, while
   only the Status line was rewritten to match the shipped code; and (b)
   RFC-0021 was not updated and `EIR_MINOR` was not bumped.
2. 0072 filed (Medium): evidence — `9fdb4515` touched only `reference/src/eir.rs`
   + `rfc/RFC-0046-…md` + two docs (`rfc/RFC-0021` untouched);
   `EIR_MAJOR/MINOR = 2/0` (eir.rs:41-42); decoder rejects any other minor
   (:2126); pre-0046 decoder rejects `block_count != 1` (error 30). Shipped
   multi-block framing (`block_count | function-level argument_count | per
   block: block_id | params(reserved) | instruction_count | instrs`,
   eir.rs:2100-2112) is not spelled out by the normative RFC → same program,
   different bytes, same declared version.
3. 0073 filed (Low): `encode()` newly routes through `blocks_from_instructions`
   (eir.rs:2090 → def :2229), which panics on degenerate input (empty stream →
   `e-1` underflow; out-of-range `Br`/`CondBr` target → `instructions[e-1]`
   OOB). Partly pre-existing (validate :1145 and the view :722/738 already call
   it); reachability from real source is low.
4. Numbering note: a concurrent reviewer took **0071** (`merge_sources` re-derives
   module declarations), so this pass filed 0072/0073.

Review pass 32 (landed `faf08619` std periodic package + fix verification of
`f38aac54`; closed 0071-0073 — 0 open issues):

1. Gates green on BOTH commits: fmt, clippy `-D warnings`, tests **399** at
   `faf08619` (+1 `periodic_table_lookups`) → **401** at `f38aac54` (+2 fix
   tests), conformance **25/25**, release builds.
2. `faf08619` black-box: all 118 `std/elements/*.pwe` compile standalone (the
   test suite itself covers Fe/F/He + the on-disk count); `std/periodic.pwe`
   compiles in ~55 ms (38 fns, 1.7 MB artifact); runtime values match the
   table (CLI probe Fe: `state=[55.8452, 8.0, 1.0]`; test covers F 3.98,
   He noble 1, O valence 6, U period 7); artifact hash stable across
   recompiles; LSP diagnostics EMPTY for a std-importing buffer and for
   `Fe.pwe`; MIT attribution + units table present in `std/elements/README.md`;
   README counts 399.
3. Privacy semantics re-checked around the package: modules without `export`
   are fully public by design (`Fe.x()` file-stem alias and
   `elements.Fe.x()` declared name both resolve); dotted partial-export fires
   102 on CLI and LSP (`foo.Qux.h2`; LSP range at 0:0 for system-body calls —
   the from-import path still positions at the item name);
   `from "<path>" import x` honors the export surface (102).
4. 0071 reproduced on `faf08619` (compile rc 0 / run false 101), then verified
   **fixed** on `f38aac54`: `pois2` probe runs (rc 0, 60 steps); a REAL
   duplicate still errors (`101 … in v101/a.pwe and v101/b.pwe`); test
   `merge_sources_ignores_module_line_inside_string`. Closed. Residual stays
   on record: `merge_sources` still hard-codes `exports: None` → 102 never
   runs on the artifact run path (pass-29 note, not filed).
5. 0072 verified **fixed** on `f38aac54`: RFC-0046 §Wire format now reads
   "— as shipped" and matches `encode_functions_section` byte-for-byte (the
   BLOCKS-section/minor-bump bullet is gone); RFC-0021 gains the concrete
   FUNCTIONS layout for both encodings + a Compatibility rationale for
   `EIR_MINOR = 0` (exact-minor decoder; a bump would reject every existing
   artifact); alignment row updated. Closed with one recorded residual:
   RFC-0046 Design/Representation still specifies the block-params target
   (`Block{params,ops,term}`) while shipped `dominance::Block` is a range
   view — Status marks params reserved/future, accepted as target design.
6. 0073 verified **fixed** on `f38aac54`: empty stream → one empty Return
   block (no `e-1` underflow), out-of-range branch targets ignored;
   test `eir_encode_does_not_panic_on_degenerate_input` (empty body +
   `CondBr [1,99,100]` → `encode().is_ok()`). Closed. No valid-source
   black-box trigger existed pre-fix (error 55 rejects empty update bodies;
   lowering always emits); the underflow/OOB at `eir.rs:2247` was confirmed
   at code level, matching the Low severity.
7. Observations (not filed): the `std/elements/README.md` example
   `import "std/periodic"` assumes the program sits at the repo root (imports
   resolve file-relative — rc 0 from root, 76 from a subdir);
   `strip_directives(&src.root)` runs twice for the root source in
   `merge_sources` (negligible).

Review pass 33 (landed `f4726072` bond/molecules + `261f75bc` native call-scope
fix; filed 0074):

1. Gates green on both commits: fmt, clippy `-D warnings`, conformance
   **25/25**, release builds; tests **402** at `f4726072`
   (+1 `bond_declaration_attaches_render_bonds`) → **403** at `261f75bc`
   (+1 `native_two_calls_in_one_function_compile`).
2. `f4726072` `bond <a> <b>` black-box: grammar is additive — an entity or a
   state slot named `bond` still parses and runs (`state=[61.0]` probes);
   malformed `bond A` gives error 60 at the failing token (3:1 = `}`); names
   that match no entity are silently dropped (presentation-only, documented);
   artifact hash stable across recompiles; `pwe fmt` preserves both `bond`
   lines in `water.pwe`; LSP diagnostics EMPTY for the demo, water, and the
   ghost-bond buffer. Merge dedup is exact-tuple — `(a,b)` vs `(b,a)` both
   kept (noted, not filed; presentation-only).
3. Molecules: all 9 `std/molecules/*.pwe` compile; `water.pwe` standalone runs
   (3 entities); `cli/examples/molecule.pwe` compiles and runs with
   `state=[18.0150, 3.0000]` (H₂O molar mass + atom count correct; `s0`/`s1`
   auto-create on an entity that declares no `state`).
4. Atomic structure: all 118 element modules compile with the +4 functions;
   probes — Fe `electrons=26, neutrons=30, shell_count=4,
   shell_electrons(3)=14`; `periodic.neutrons(26.0)=30`; `std/periodic.pwe`
   compiles.
5. 0074 filed (Low): both READMEs still say 399 (badge, `cargo test` comment,
   table totals; unit row 347) while the workspace is 402 — `faf08619`
   refreshed counts by convention, `f38aac54` and `f4726072` did not.
6. `261f75bc` verified: the Call arm now wraps each call in its own C block so
   `t{i}`/`ta[]` temporaries cannot collide across two calls in one block.
   Black-box: a two-call program (`f(a,b) { g(a) + g(b) }`) run with
   `--steps 200` (crosses `NATIVE_PROMOTE = 64`) — native-on rc 0,
   `x = 10.0`, output identical to `--no-native-jit`; regression test asserts
   `native.call(id, [3,4]) == 14.0` (skips when `cc` is absent).
7. Observation (not filed — in-flight): `f4726072`'s message claims
   "tests (… atomic structure, molecule molar mass/bonds)" but those stdlib
   tests exist only as uncommitted working-copy WIP
   (`reference/tests/stdlib.rs`, +44 lines); the commit itself contains only
   the bond test. Will file if a subsequent commit lands without them.

Local copies of the bodies live next to this file (`0001-…` … `0035-…`).
