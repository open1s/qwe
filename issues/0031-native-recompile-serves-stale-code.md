# 0031 — Second `NativeProgram::compile` in one process silently executes the first program's code (fixed temp path + dlopen path cache) (High)

## Summary

`NativeProgram::compile` (`reference/src/native.rs`, commit `7cc2d5a5`)
always writes to the **same files**:

```rust
let dir = std::env::temp_dir().join(format!("pwe_native_{}", std::process::id()));
// ... dir/pwe_native.c , dir/libpwe_native.dylib
let handle = unsafe { dlopen(cpath.as_ptr(), RTLD_NOW) };
```

Within one process, every compile targets
`$TMPDIR/pwe_native_<pid>/libpwe_native.dylib`. The second compile
overwrites a file whose image is already loaded; `dlopen` on the same
path returns the **cached first image**. The second `NativeProgram`
then executes the *first* program's machine code while reporting
success.

## Evidence

Probe (commit `7cc2d5a5`, macOS, fresh process):

```text
prog1: funcs { h(z) { z + 1.0 } }
prog2: funcs { h(z) { z + 2.0 } }
n1 = NativeProgram::compile(prog1); n2 = NativeProgram::compile(prog2);
n1.call(id, &[10.0]) = Ok(11.0)   // correct
n2.call(id, &[10.0]) = Ok(11.0)   // WRONG: expected 12.0 — prog1's body
=> BROKEN: second compile silently reuses/overwrites the loaded image
```

The same collision fired incidentally in another probe: after a `sign`
module was compiled, compiling a `div` module and calling it returned
`1.0` (= `sign(1.0)`) instead of `1.0/0.0`.

Secondary defects in the same code:

- The temp directory is **never removed** (`Drop` only `dlclose`s) —
  verified `$TMPDIR/pwe_native_*` accumulates one dir per process.
- Predictable per-pid path in a shared temp dir is a classic
  symlink/pre-creation hazard on multi-user systems (`create_dir_all`
  follows a planted symlink → writes follow it).

## Impact

Silent wrong results (not an error path) the moment the backend is
wired to run more than one program per process — the normal case for a
server/REPL. Combined with #32/#33/#34, stale-code execution makes
differential behavior nondeterministic across calls.

## Suggested fix

Make the artifact path unique per compile and identity-bearing
(RFC-0035 style): e.g. `pwe_native_<pid>_<n>_<artifact_hash>.dylib`,
create with `0700`, and remove the directory on `Drop` (or keep behind
a debug env flag). Include the EIR/artifact hash in the filename so a
same-path dlopen is impossible for different content, and identical
content maps to a cache hit deliberately.
