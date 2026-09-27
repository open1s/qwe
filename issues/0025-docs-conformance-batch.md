# 0025 — Docs conformance: stale example output, contradictory Appendix B, undocumented section order, lenient color parsing (Low)

## Summary

A black-box conformance pass against `docs/lang-usage.md` found four doc
mismatches (and re-verified two claims as correct).

### 1. L1 oscillation example reports the wrong result

docs/lang-usage.md:149 claims:

> Run: `pwe run osc.pweb --steps 200` → `state = [0.7226, -1.1813]`.

Actual (docs' own example source, `k=12, c=0.4, dt=0.01`):

```
$ pwe run osc.pweb --steps 200
  #1  m  state = [0.6302, -1.5751]
```

The implementation is right and the docs number is stale: an independent
simultaneous-Euler reference of the documented system reproduces
`[0.6302, -1.5751]` exactly (sequential orderings give `[0.5532, -1.3909]`
/ `[0.5671, -1.3909]` — neither matches the docs either).

### 2. Appendix B still teaches the pre-v0.3 idiom

docs/lang-usage.md:901 (stability checklist):

> Rules are `slot = <expression>`; assign with `slot = (target - slot)`.

This contradicts the v0.3 semantics stated at
docs/lang-usage.md:172-173 ("**Assign vs integrate.** Since `=` assigns, a
constant write is just `slot = target` — no idiom needed") and the fix
landed for #1. New users following the checklist will write subtraction
rules that *add* the residual forever.

### 3. Required section order is undocumented

The grammar mandates `world → funcs? → systems?`
(`reference/src/lang.pest:8`), but no doc states it
(`rg 'section order|funcs.*after.*world|world.*first' docs/ README*` →
empty). A `funcs`-first file fails with a grammar-internal message:

```
$ pwe compile so.pwe -o so.pweb     # funcs { … } before world { … }
error 60: failed to parse program:  --> 1:1
  |
1 | funcs { f(a) { a } }
  | ^---
  |
  = expected world_section
```

(The parser names the *internal* rule, not the user-facing section.)

### 4. Color literals: length unvalidated, docs say `0xRRGGBB`

Grammar accepts any run of hex digits (`reference/src/lang.pest:89
color = @{ "0x" ~ ASCII_HEX_DIGIT+ }`), while docs/lang-usage.md:383 and
:498 both specify `0xRRGGBB`:

| literal | length | result |
| --- | --- | --- |
| `0x12345` | 5 | compiles silently |
| `0xFF6B4ACC` | 8 | compiles silently (RGBA? undocumented) |
| `0x123456789` | 9 | `Invalid (64): invalid color literal` |

Detail 64 only fires via `u32` overflow — it is a width check disguised as
a literal check, so 5/7-digit values pass with an implicit leading-zero
interpretation and 8 digits are accepted without documentation.

### 5. (clarification) `linear` formula notation

docs/lang-usage.md:601 gives `s_N' = Σ a_j s_j + c`. Read as a *derivative*
this matches the implementation (explicit Euler `s += dt·(A s + c)`:
lin.pwe with rows `(1,0,1),(0,1,1)`, `dt=1`, 3 steps → `[15.0, 23.0]`,
consistent with the radioactive-decay expectation
`N0·(1−λ)^n` in `reference/src/lang/tests.rs:185-235`). Read as a
*next-state map* it is off by `dt` and the identity term. Suggest writing
`ds/dt = A s + c` (and noting the Euler step) to remove the ambiguity.

### Claims re-verified as correct (no action)

- docs/lang-usage.md:315 L5: "64 particles active after 40 steps" —
  `pwe run l5.pweb --steps 40` → `entities: 65` = 1 emitter + 64 pool
  slots, all falling. ✓

## Evidence

- Probes: `/tmp/pwe_final/{osc,l5,so,c5,c8,c9,lin}.pwe` (re-verified on
  main `9d6277fe`).
- Docs: lang-usage.md:149, :315, :383, :498, :601, :633+ (§2.9),
  :172-173, :901.
- Grammar: `reference/src/lang.pest:8` (section order), `:89` (color).

## Impact

The headline example output is wrong (users comparing their run will
think they broke something), the Appendix B checklist actively teaches the
superseded idiom #1 removed, and color/section-order gaps produce
either silent behavior differences or intimidating grammar errors.

## Suggested fix

Update the four doc spots (L1 number, Appendix B line, a one-line
"sections must appear in `world`, `funcs`, `systems` order" note in §2.10,
and the `0xRRGGBB` description to state accepted lengths — or enforce
exactly 6/8 hex digits at parse and say so); rewrite the `linear` formula
as a derivative.
