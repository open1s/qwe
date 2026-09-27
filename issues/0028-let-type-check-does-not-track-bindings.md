# 0028 — `let` type check never types names: aliasing a bool-typed binding is wrongly rejected (89), integer annotations are vacuous (Low)

## Summary

The detail-89 check from `b682e692` classifies expressions purely by
syntax (`reference/src/lang/parser.rs:81-87`):

```rust
pub(crate) fn expr_lang_type(e: &Expr) -> LangType {
    match e {
        Expr::Cmp(..) | Expr::And(..) | Expr::Or(..) | Expr::Not(..) => LangType::Bool,
        _ => LangType::Float,   // ← Expr::Name is always "number"
    }
}
```

Bindings established by earlier `let`s in the same system are not
tracked, so the check both **rejects correct programs** and **rubber-
stamps wrong ones** whenever the right-hand side is a bare name.

## Repro

False positive — aliasing a condition is rejected:

```pwe
world { entity e { state = (x=1.0) } }
systems { update { on=e; dt=1.0
 let a: bool = x > 0.0
 let b: bool = a
 y = b
} }
```

```
$ pwe compile nameref.pwe -o nameref.pweb
error 89: `let b: bool` but the expression is number
```

`a` is a bool-typed binding; `let b: bool = a` is exactly what the
annotation says. (Note also: this error prints with **no source
location** — bare line, no caret; see the note in #27.)

False negative — the inverse direction is silently accepted:

```pwe
systems { update { on=e; dt=1.0
 let b: bool = x > 0.0
 let c: f64 = b + 1.0     # bool used where f64 claimed
 x = c
} }
```

```
$ pwe compile rev.pwe -o rev.pweb
compiled rev.pwe -> rev.pweb   (rc=0)
```

Because `b + 1.0` is syntactically numeric, `f64` passes; because `b`
alone would be typed "number", `let c: f64 = b` also passes and
`let d: bool = b` would be *rejected*. For name right-hand sides the
annotation is effectively decorative — and it is wrong in both
directions.

## Repro (secondary): `type_name` matches prefixes

```pwe
let a: boolean = x > 0.0
```

```
$ pwe compile prefix.pwe
error 60: failed to parse program:  --> 3:2
  |
3 |  let a: boolean = x > 0.0
  |  ^---
  |
  = expected unit_expr, param, or_op, and_op, cmp_op, add_op, or term_op
```

`type_name = @{ "f64" | "i64" | "i32" | "u64" | "u32" | "bool" }`
(`reference/src/lang.pest:153`) has no word boundary, so `boolean`
consumes `bool` and the failure surfaces as a confusing expression-level
parse error pointing at `let` rather than "unknown type `boolean`".

## Evidence

- `reference/src/lang/parser.rs:74-87` — `LangType` / `expr_lang_type`;
  `:97-115` — `parse_let_parts` (no binding env, `Expr::Name` → Float via
  the `_` arm; integer annotations `i64|i32|u64|u32` share the `Float`
  arm, so `let n: i64 = 1.5` also passes).
- `reference/src/lang.pest:153` — `type_name` without boundary.
- Probes: `/tmp/pwe_rev1/{nameref,rev,prefix}.pwe`, verified on main
  `b682e692`.
- Commit message is honest that the annotation is "documentation + a
  check", but docs/lang-usage.md:646-648 present it as a bool/number type
  check without the "names are untracked" caveat.

## Impact

The false positive is user-facing: a natural two-line pattern (bind a
condition, then alias it) fails to compile with a message that contradicts
the code. The false negatives mean the check gives false confidence for
any rule that mixes named bindings — the common case in longer systems.
The prefix parse error and the vacuous integer types add avoidable
confusion while the feature is new and fresh in users' minds.

## Suggested fix

1. Thread a small `HashMap<&str, LangType>` of already-bound `let`s
   through the sequential statement parse (both in `store_param`'s
   `update_stmts` walk and the funcs path); type `Expr::Name` from it,
   falling back to `Float` only for genuinely unknown names (which
   separately get warning 85 at lowering).
2. Give `type_name` a word boundary (e.g. `@{ (ASCII_ALPHA | "_")+ }`)
   and emit detail 89 (or a 60 with the actual token) for unknown
   annotations instead of a mis-cored expression parse error.
3. Either implement the integer types' promise (literal/integer-ness
   check) or document them as aliases of `f64` pending i64 support.
4. Extend `let_type_annotations_are_checked` with the alias case, the
   `boolean` typo, and an integer-annotation case.
