Reported while verifying `03170cc5` (pass 18); previously noted in the #42 close comment.

`reference/src/gpu.rs:63-64` (`Gpu::new` doc comment):

```rust
/// Creates the Metal device and compiles the stencil kernel. Errors (detail
/// 5) if there is no Metal device or the kernel fails to compile.
```

The code no longer returns detail 5 anywhere on this path:

- no Metal device → detail **98** ("no Metal GPU device available")
- MSL library / `get_function` / pipeline failure → detail **99** ("Metal kernel failed to compile")

(`03170cc5` split 98/99 precisely because of this; only the doc line was missed.)

Expected: the doc comment cites 98/99. Impact: a public API doc contradicting the actual error contract — anyone matching `detail == 5` per the docs will never see it.
