# 0027 — Annotated `let` in loop bodies is silently swallowed: initializer replaced by the type name, detail 89 never checked (Medium)

## Summary

Commit `b682e692` ("typed `let` bindings, detail 89") routed the two main
`let` sites through the new `parse_let_parts` helper — the top-level
`store_param` path (`reference/src/lang/parser.rs:132`) and the `funcs`
statement path (`parser.rs:1306`) — but **missed the loop-body path**.
`build_loop_body` (`parser.rs:444-461`) still extracts children the old
way:

```rust
Rule::let_stmt => {
    let mut li = inner.into_inner();
    let name = next_pair(&mut li)?.as_str().to_string();
    check_let_name(&name, offset)?;
    let expr = next_pair(&mut li)?.as_str().trim().to_string(); // ← 2nd child
    body.push(UpdateStmt::Let(name, expr));
}
```

With the grammar now producing `[ident, type_name, expr]` for annotated
lets, "the expression" is read from the **type-name pair**. Two failures:

1. The real initializer is **discarded** — `let v: f64 = 5.0` becomes
   `let v = f64`, so `v` reads 0.0 (with a misleading unknown-identifier
   warning for the type name itself).
2. The detail-89 check inside `parse_let_parts` **never runs**, so an
   invalid annotation compiles cleanly.

`repeat` and `for` bodies are affected (both go through
`build_loop_stmt` → `build_loop_body`, including nested loops).

## Repro

```pwe
world { entity e { state = (x=1.0) } }
systems { update { on=e; dt=1.0
 repeat 2 { let v: f64 = 5.0 }
 x = 0.0 + v
} }
```

```
$ pwe compile loop_ann.pwe -o loop_ann.pweb
compiled loop_ann.pwe -> loop_ann.pweb
warning [85]: unknown identifier `f64` — reads 0.0 (typo?)
warning [85]: unknown identifier `f64` — reads 0.0 (typo?)
```

Expected: compiles silently with `v = 5.0`. Actual: `v` is `f64`-the-
identifier → 0.0, and the warning accuses the *type name* of being an
unknown identifier (printed once per unroll).

Mismatch case — must be detail 89, compiles instead:

```pwe
world { entity e { state = (x=1.0) } }
systems { update { on=e; dt=1.0
 repeat 2 { let v: bool = 5.0 }
 x = 0.0 + v
} }
```

```
$ pwe compile loop_mismatch.pwe -o x.pweb
compiled loop_mismatch.pwe -> x.pweb
rc=0
warning [85]: unknown identifier `bool` — reads 0.0 (typo?)
warning [85]: unknown identifier `bool` — reads 0.0 (typo?)
```

Contrast — same mismatch at top level correctly fails:

```
$ pwe compile sys_mismatch.pwe   # let b: bool = 1.0
error 89: `let b: bool` but the expression is number
```

## Evidence

- `reference/src/lang/parser.rs:444-461` — `build_loop_body`'s
  `Rule::let_stmt` arm still does the two-`next_pair` extraction;
  `parser.rs:129-137` (store_param) and `:1306-1309` (funcs) were updated
  to `parse_let_parts`, proving the intended pattern.
- `reference/src/lang.pest:151` — `let_stmt = { "let" ~ ident ~ (":" ~
  type_name)? ~ "=" ~ expr }` applies everywhere `let_stmt` is used,
  including loop items.
- Probes: `/tmp/pwe_rev1/{loop_ann,loop_bool,loop_mismatch,sys_mismatch}.pwe`,
  verified on main `b682e692` (fresh `cargo build --workspace`).
- Tests: `reference/src/lang/tests.rs::let_type_annotations_are_checked`
  only exercises top-level update-system lets — the loop path (and funcs
  path) have no coverage, which is why this slipped through.

## Impact

The new feature is broken in loop bodies in the worst way: the program
still compiles, but the bound value is silently wrong (0.0 instead of the
initializer) and the type system is not enforced. Silent wrongness in
exactly the code (`repeat { let … }` fixed-point/Newton loops) where
correctness matters most. One of the two failure modes also generates a
misleading warning that blames the type name for being an unknown
identifier.

## Suggested fix

Replace the inline extraction with the helper (same as the other two
sites):

```rust
Rule::let_stmt => {
    let (lname, text) = parse_let_parts(inner, offset)?;
    body.push(UpdateStmt::Let(lname, text));
}
```

Add regression tests for: annotated let in `repeat` (value correct),
annotated let in `for`, and a mismatching annotation inside a loop →
detail 89. Consider one test for the `funcs` path too (it was updated but
is untested).

### Note (pre-existing, not introduced here)

Detail 89 (like parse-time detail 65) prints without a source location —
`error 89: …` bare, no `--> line/column` — because parse-time errors go
through the `diagnose("")` path (`cli/src/main.rs:323`). Same family as
#16/#20; worth fixing when that is addressed.
