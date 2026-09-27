# 0008 — Reserved-word table is incomplete (Low)

## Summary

The reference lists keywords that "cannot be used as identifiers" but omits
`else`, and omits the statement forms `inte` / `deriv` / `+=` from the surface
syntax the reader has to know.

## Evidence

- Table: `docs/lang-usage.md:506-520` — control list is `return let repeat
  until while for in break continue if`; **no `else`**.
- Grammar reserves it: `reference/src/lang.pest:27` —
  `if_stmt = { if_kw ~ expr ~ "{" ~ if_body ~ "}" ~ (else_kw ~ "{" ~ if_body ~ "}")? }`,
  `reference/src/lang.pest:29` — `else_kw`.
- `inte` / `deriv` are operators with arity checks
  (`reference/src/lang/parser.rs:723`, `mod.rs:1452`) and are taught only in
  prose (`docs/lang-usage.md:159-164,685`), not in §2.2.
- `+=` is a distinct grammar statement (`add_stmt`, handled at
  `parser.rs:212-226`) absent from the keyword/precedence section.

## Impact

`entity else { … }` / `field if { … }` are latent parse conflicts; a user who
trusts the reserved-word table will hit an opaque detail-60/55 parse failure
instead of "reserved word". The missing `inte`/`deriv`/`+=` entries make §2.2
look like the complete syntax when it is not.

## Fix

1. Add `else` to the control keyword list in `docs/lang-usage.md:517-518` and
   verify the lexer rejects `else` as an identifier everywhere (grammar-level
   `keyword` rule rather than context-only `else_kw`).
2. Add a "statement forms" row to §2.2 covering `ident = expr`,
   `inte ident = expr`, `ident += expr`, `s[i] = expr`, call statements.

## Labels

docs, grammar, low
