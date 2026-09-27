# 0005 — Doc drift: soft bodies and joints are shipped, README says otherwise (Medium)

## Summary

The README's limits section still claims soft bodies and joints are missing
although both are implemented, tested, documented, and shipped as examples.
Other headline numbers (test count) disagree across files.

## Evidence

- Stale claim: `README.md:270` — "soft bodies or a full joint family yet";
  `README-ZH.md` carries the same section.
- Implemented: `reference/src/lang/mod.rs:2274` (`JointSystem`), `:2689`
  (`SoftSystem`), `:4843` (`"soft"` kind), `:4930` (`"joint"` kind);
  `reference/src/lang/parser.rs:177-178` (kind param tables for `soft`/`joint`).
- Documented and taught: `docs/lang-usage.md:317-354` (L6 joints, L7 soft
  bodies), `:607-608` (system table), `:842-843` (`chain.pwe`, `cloth.pwe`,
  `jelly.pwe`).
- Number drift: `README.md` badge says 304 tests; roadmap docs say 306;
  `rfc/` holds 40 RFCs while the badge text quotes an older count.

## Impact

Understates the product, and — more importantly for this review — the same
stale text was copied into downstream docs (wiki FAQ, `docs/book/faq.md`), so
the error propagates.

## Fix

1. Sweep `README.md` / `README-ZH.md` limits + badges against `rfc/` and
   `cargo test` output; regenerate numbers from a script rather than editing by
   hand.
2. Re-check wiki `FAQ` and `docs/book/faq.md` (both authored during this
   session) for the same claims and correct them.

## Labels

docs, accuracy, medium
