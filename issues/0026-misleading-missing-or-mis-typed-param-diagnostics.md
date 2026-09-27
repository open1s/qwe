# 0026 — Misleading parameter diagnostics: present-but-wrong-kind reported as "missing"; missing `chan` reported as "unknown name" (Low)

## Summary

Two parameter-lookup paths turn one mistake class into the wrong error:

### 1. `watch { into = flag }` → "missing required parameter 'into'"

`into` **is** present; its value is just an identifier instead of a
number. The parser routes identifier-valued parameters to
`string_params` (`reference/src/lang/parser.rs:168-169 Rule::ident =>
decl.string_params.insert(...)`), while the compiler reads `into` from
the numeric map (`reference/src/lang/compile.rs:444-445` via `param()`,
whose table does list `into` for `watch` —
`numeric_param_keys("watch") = ["mem", "into"]`). Numeric lookup misses
and `param()` reports detail 48 "missing".

```
$ pwe compile w1.pwe
error 48: system 'watch' is missing required parameter 'into'
  --> line 2, column 11
```

with `w1.pwe`:

```pwe
world { entity e { state=(x=1.0) } }
systems { watch { on=e; expr=1.0; mem=0; into=flag } }
```

The same system with `into = 0` compiles cleanly (`w2.pwe`), confirming
the parameter name is known and the actual complaint is the *value kind*.

### 2. `send` without `chan` → "unknown entity or channel name"

```pwe
world { entity e { state=(x=1.0) } }
systems { send { on=e; value=1.0 } }
```

```
$ pwe compile send1.pwe -o send1.pweb
Invalid (62): unknown entity or channel name
```

The missing key and the unknown name share one error site
(`reference/src/lang/compile.rs:124-128`):

```rust
let chan_name = s.string_params.get("chan").cloned()
    .ok_or(error(Status::Invalid, 62))?;
```

Docs distinguish the two: 48 = "Missing required system parameter"
(docs/lang-usage.md:777), 62 = "Unknown entity or channel name"
(docs/lang-usage.md:787). Every other required parameter goes through
`param()`, which emits 48 with the system name and a source offset —
`send`'s `chan` does not, so it also prints with no location.

## Evidence

- `reference/src/lang/parser.rs:167-169` — identifier RHS →
  `string_params`; `reference/src/lang/parser.rs`
  `numeric_param_keys("watch")` — `into` is a numeric param name.
- `reference/src/lang/compile.rs:26-34` — `param()` (48 via `error_at`);
  `:444-445` — watch's `mem`/`into` reads; `:124-128` — send's `chan`
  read mapped to bare `error(62)`.
- Probes: `/tmp/pwe_final/w1.pwe` (into=flag → 48), `w2.pwe`
  (into=0 → compiles), `send1.pwe` (no chan → 62), re-verified on main
  `9d6277fe`.
- Docs: lang-usage.md:777 (48), :787 (62), :607 (`watch` params row).

## Impact

Both messages send the user in the wrong direction: they will hunt for an
`into` they already wrote, or for a mistyped channel name they never
provided. Small stuff, but it is exactly the "diagnostic quality" axis
#16 covered — and the fix-48 path already exists, just not wired for
wrong-kind values or `send.chan`.

## Suggested fix

1. In `param()` (or at each call site), check `string_params` before
   declaring "missing": if the key exists there, report
   "`into` expects a slot index (number), got identifier `flag`"
   (new detail or a 48 variant with the value text).
2. Split `send`'s `chan` handling: absent key →
   `error_at(48, s.byte_offset, "send is missing required parameter
   'chan'")`; present-but-unresolvable → keep 62 with the name.
3. Regression tests for both repros asserting the message text.
