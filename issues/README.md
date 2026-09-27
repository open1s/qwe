# PWE issues

Language-design review findings, filed from the review of `lang.pest`,
`reference/src/lang/{parser.rs,mod.rs}`, `docs/lang-usage.md`, and the READMEs.
Every finding carries `file:line` evidence. Nothing here is committed upstream.

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

## Suggested order

1. 0001 — actively misleading, one-line fixes.
2. 0002 + 0009 — one new detail code each; removes a class of silent wrongness.
3. 0003 + 0006 — name/layout validation with diagnostics.
4. 0004, 0007, 0008, 0010 — documentation passes; 0005 — stale-limits sweep
   (include wiki FAQ and `docs/book/faq.md`).

Local copies of the bodies live next to this file (`0001-…` … `0010-…`).
