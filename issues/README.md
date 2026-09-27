# PWE issues

Findings from two passes: (1) a language-design review of `lang.pest`,
`reference/src/lang/*`, `docs/lang-usage.md`, and the READMEs (0001–0010);
(2) a black-box testing pass with compile/run probes (0011–0017). Every
finding carries `file:line` evidence and a reproduction. All are filed
upstream at github.com/open1s/qwe/issues. (3) a verification pass against
the developer fix commit `1df656bb`: 0001–0015 verified fixed except
0011 (reopened: `on`-less `send`/`recv` still corrupts, docs omit `on`) and
the 0003 residual, filed as 0018; 0016 verified; 0017 still open by design.

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

Local copies of the bodies live next to this file (`0001-…` … `0018-…`).
