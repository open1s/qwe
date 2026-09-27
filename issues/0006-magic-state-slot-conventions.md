# 0006 — Magic state-slot conventions with silent failure (Medium)

## Summary

Several system kinds interpret state slots positionally (mass at slot 6, euler
orientation at slots 7/8/9), and violating the convention does not produce a
diagnostic — the system just behaves differently or does nothing.

## Evidence

- `nbody` layout: `docs/lang-usage.md:233` — "mass in **slot 6**, *not* the
  `mass` field"; `:596` — `state=(px,py,pz,vx,vy,vz,m)`; `:695-696` — "`nbody`
  mass is `state[6]`. `state[7]` is a Z-spin unless `orient = true`."
- Orientation convention: `docs/lang-usage.md:401-413` (slots 7/8/9 as
  pitch/yaw/roll only when `orient = true`), `:469` ("Angle/spin looks wrong …
  `state[7]` is a Z-spin by default").
- Related failure is only detectable at runtime/by inspection:
  `docs/lang-usage.md:464` — "`nbody` does nothing → mass not in `state[6]`";
  detail 63 (`reference/src/lang/diagnostics.rs:85`) only fires when there are
  *no dynamic bodies*, not when the layout is wrong.

## Impact

A user who writes `entity earth { state = (px, …, mass = 1.0) }` with 7 slots
but wrong ordering, or adds an 8th slot, gets plausible-but-wrong physics with
no error. Combined with F2 (unknown names read 0.0), these conventions are the
main "silent wrongness" surface of the language.

## Fix

1. For `nbody`, require/validate the named layout `state=(px,py,pz,vx,vy,vz,m)`
   (or accept the names as aliases) and emit a diagnostic when slot count/order
   does not match — new detail code or reuse 80.
2. Warn when slots 7/8/9 are written by rules while `orient != true`
   (and vice versa), since the interpretation silently changes.
3. Mention both checks in the stability checklist (`docs/lang-usage.md:874`).

## Labels

diagnostics, language-design, medium
