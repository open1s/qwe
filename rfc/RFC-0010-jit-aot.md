# RFC-0010: JIT and AOT

**Status:** Draft \| **Version:** 0.1.0

## 1. Modes

-   Interpreter: debug, editor, cold start, dynamic experimentation.
-   JIT: hot paths, runtime specialization, dynamic worlds.
-   AOT: stable deployments, edge, robotics, servers.

## 2. Hotness

Cold -\> Warm -\> Hot -\> VeryHot.

## 3. Code cache key

``` rust
pub struct CodeCacheKey {
    pub module_hash: Hash,
    pub target: TargetId,
    pub profile: ProfileHash,
}
```

## 4. JIT lifecycle

Compile -\> Validate -\> Install -\> Execute -\> Profile -\>
Recompile/Invalidate.

## 5. Assumptions

Specialized code must record assumptions such as component schema,
entity count range, constant values and device capabilities.

## 6. Deoptimization

If an assumption becomes false, execution must safely fall back to
generic EIR/interpreter or another valid compiled version.

## 7. Reference backends

Cranelift for fast CPU JIT; LLVM for high-optimization AOT;
wgpu/SPIR-V/WGSL and native GPU backends for device targets.

## 8. Safety

Unvalidated generated code cannot enter production execution.
