# 0030 — `NativeProgram::call` has no arity validation: safe API reads out of bounds and SIGSEGVs (High)

## Summary

`reference/src/native.rs` (commit `7cc2d5a5`) exposes

```rust
pub fn call(&self, id: u64, args: &[f64]) -> Result<f64>
```

The emitted C function (`double pwe_f_<id>(const double* a)`) reads
`argument_count` doubles from `a` unconditionally (`r[i+1] = a[i]` for
`i in 0..argument_count`), but `call()` never checks `args.len()` against
the function's arity. `NativeProgram` doesn't even store argument counts
(only `compiled: Vec<u64>`), so it *cannot* validate.

## Evidence

Probe (commit `7cc2d5a5`, macOS, fresh process):

```text
before call            <- eprintln before np.call(ID, &[])
exit=139               <- SIGSEGV
```

`np.call(0xf000_0000, &[])` on a 1-argument function crashes the
process. The empty slice's `as_ptr()` is `0x8` (alignment of `f64`);
the generated code dereferences it → SIGSEGV. A short-but-nonempty
slice (e.g. `&[1.0]` for a 2-arg function) reads adjacent stack instead
— silent garbage, no error.

This is a *safe* Rust function violating AGENTS §11: the unsafe
invariant of the C ABI ("caller provides `argument_count` doubles")
is not enforced behind the safe abstraction, and the required
invariant documentation is absent.

## Impact

Any future caller (JIT tier, world-access kernels in the WIP, tests,
user code) that gets an arity wrong gets a process crash or silent
garbage instead of `Err`. Crash-class bugs are the worst failure mode
for a runtime meant to trap and report (details 17/18 style).

## Suggested fix

Store `(id, argument_count)` pairs in `NativeProgram` (it already has
the `Function`s at compile time), and in `call()`:

```rust
let want = self.arity(id).ok_or(...)?;
if args.len() != want { return Err(error(Status::Invalid, /*detail*/ 5)); }
```

Add `# Safety`-style docs on the ABI contract and a regression test:
`call(id, &[])` and `call(id, &[…; n+1])` must return `Err`, never trap.
