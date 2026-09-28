# PWE 全面评审与改进路线图

> 评审视角：编程语言设计 + 系统架构 + 计算科学 + 性能工程。
> 依据：仓库实测（LOC、依赖、语法、RFC、测试、clippy/fmt/conformance、基准）。

## 0. 总评

PWE 方向正确：**微内核 + 分层 IR（WIR → Domain IR → EIR）+ 确定性优先 + 冻结 ABI + 能力模型**，
已能表达 4D 物理/化学/生物仿真，并有可用的实时 3D 查看器。当前处于“**广度高、深度浅**”阶段：
语义仍在演进；核心模块膨胀；JIT/AOT 无原生代码；缺性能度量与 CI；非测试路径存在 `unwrap`。

定位：**优秀的“可编程物理世界”确定性与原型平台，尚未成为生产级通用科学计算语言。**

## 1. 现状度量

| 指标 | 值 |
| --- | --- |
| Rust 总 LOC | ~37.8k（reference 31k） |
| 最大文件 | `reference/src/lang.rs` 10.8k 行；`eir.rs` 3.4k；`physics_eir.rs` 2.2k |
| 外部依赖 | 仅 `pest`（+ 宏传递依赖）/ 无 `no_std` 之外的重依赖 |
| `pwe-api` | `#![no_std]`、零依赖、39 公开项、`include/pwe_abi.h` |
| RFC | 40 份；conformance 仅 17 用例 |
| 测试 | 306 个（含 laws 17） |
| 非测试 `unwrap()` | 132（lang.rs 66、snapshot.rs 13…） |
| `unsafe` | 9 |
| JIT/AOT | 无原生代码（“validated cache / frozen program”） |
| CI/基准 | 无（本次新增） |

## 2. 分维度评估

### 2.1 代码实现与架构
- 优点：依赖极简、契约分层清晰、`step_cross` 逐字节差分、ordinal 稳定。
- 问题：`lang.rs` 单体化；**EIR 指令元数据在 5 处手工同步**（enum/`from_u16`/`result_type`/`validate`/`execute`）；
  132 处非测试 `unwrap`；无 CI/工具链固定/`cargo-deny`。

### 2.2 语法与语言设计
- 优点：领域关键词清晰；单位词法无歧义；`1/60` 分数；`vecN`/`struct` 铺平；`=`/`inte`/`deriv(E)` 语义已自洽。
- 问题：`funcs` **位置式 `s0..` 形参**；表达式靠 PEG 顺序消歧、脆弱；**仅 f64 用户可见**（无 int/bool/数组/字符串）；
  `import` 仅相对路径、无命名空间；语义尚未冻结（无版本 pragma）。

### 2.3 科学正确性
- 优点：`tests/laws.rs` 17 项解析解校验；std 覆盖 12 域。
- 问题：**引擎缺少通用的“系统内同步屏障”**（曾导致 nbody 破坏牛顿第三定律）；
  物理合理性无自动诊断（守恒/CFL）；单位检查是渐进式、强度低。

### 2.4 功能完备性
- 已具备：解释器、跨后端、EIR 二进制/快照、分布式所有权、能力、池化、关节、软体、结构体、PDE 场、通道、插件 ABI 草案、查看器。
- 缺口：真实 JIT/AOT；数组/记录外类型；模块系统；LSP/fmt/REPL；GPU 计算；形式化 SSA CFG。

### 2.5 易用性
- 三命令 CLI、双语教程文档、带定位诊断。
- 缺：REPL/fmt/LSP；`funcs` 位置参数；相对导入；文档片段无自动校验。

### 2.6 稳定性与安全
- 132 非测试 `unwrap` + 9 `unsafe`；无 fuzz/Miri/proptest；浮点确定性策略未成文未测试。

### 2.7 性能（本次新增基准，已量测）
- 既有优化：批量场 opcode、稠密覆盖层、`step_cross_batched`。
- **实测问题**：`compile(nbody 64) ≈ 0.7 s`，且 **O(n²)**。定位到 `EirModule::validate`：
  每函数 `validate_function` ~4.3 ms/1605 指令，主导编译时间；根因是
  `Call` 的 **O(函数数) 线性查找** 与 `BTreeMap` SSA 表的高常数。（`build_with_guards`/clone 亦为 O(指令数)=O(n²)，nbody 固有。）
- 无真实优化后端；无回归门禁。

### 2.8 可扩展性
- IR 为扁平 opcode（非显式 SSA CFG，虽宣称 SSA-like）；opcode 元数据分散；RFC↔conformance 覆盖不均。

## 3. 路线图

### Phase 0 — 工程基座（进行中）
- [x] CI（fmt/clippy `-D warnings`/test/conformance/examples/bench）+ `rust-toolchain.toml` + `deny.toml` + `.cargo/config.toml`
- [x] 手写基准 `reference/benches/throughput.rs`（无重依赖）
- [x] 消除非测试 `unwrap()`（134→0；固定长转换用 `.expect("proven")`，解析用 `next_pair` 传播，锁用 `unwrap_or_else(into_inner)`）；以 `clippy.toml` + `#![cfg_attr(not(test), deny(clippy::unwrap_used))]` 防回归；FFI 类型别名补安全契约说明
- [ ] **系统内同步屏障（引擎通用语义）**：见 Phase 2 第 10 项（本次已提前完成核心）
- [x] 编译期性能：`validate_function` 去 O(F) 查找、SSA 表改稠密数组（compile(nbody 64) 96→38ms）

### Phase 1 — 语言与运行时定型/拆解
- [x] 拆分 `lang.rs`（11077→mod.rs 387）：`systems.rs`/`tests.rs`/`compile.rs`/`lower.rs`/`parser.rs`/`runtime.rs`/`ast.rs`/`diagnostics.rs`
- [ ] opcode 元数据单一事实源 + 全 opcode round-trip 测试
- [x] 语言版本 pragma（`world { lang_version = "0.3" }`，detail 83）**+ 迁移器**：`pwe migrate` / `lang::migrate_v02_to_v03`（旧隐式 `=` 积分与旧 `deriv` → `inte`）
- [ ] `funcs` 具名形参

### Phase 2 — 类型系统与科学正确性
- [x] 系统内同步屏障（`Barrier` = 系统边界；committed = start-of-system）
- [~] 值类型：`let` 类型标注（89）、整型常量语义、显式转换 `i64/f64/bool(…)` 已交付；**int/bool 直达 EIR 寄存器**见 RFC-0043（Proposed）
- [ ] 物理合理性运行时（守恒/CFL 诊断、内建已验证积分器）
- [ ] 物理 demo 断言纳入 CI

### Phase 3 — 高性能与多后端
- [x] 解释器读路径优化：缓存规范组件 id（免 OnceLock 原子）、`pending`/`committed` 空覆盖快路径、覆盖表 BTreeMap→HashMap；缓存调用索引（`CallIndex`，免每步排序+哈希）。既有形态本已是"预译码"（扁平 `Vec<Instruction>` + 稠密寄存器 + 跳表分派）。
- [x] 优化执行层（AOT/JIT 共用）：`EirModule::optimize` 做**常量折叠 + `Mul/Add→Fma` 超指令融合**（新增 opcode `Fma=235`，语义 = `(a*b)+c` 两次舍入，**位等价**）。`AotProgram::compile` 于编译期优化；`LangRuntime` 用优化模块执行、`jit` 保持通用 → `step_cross` 差分验证优化器。nbody-64 步进 ~7%；`Fma` 融合使算术指令 2048→784。
- [x] 真原生 AOT/JIT（`native.rs` + `NATIVE_TARGET`）：纯 `funcs`（无世界访问）与**世界访问型内核**（`ReadView`/`ReadCommitted`/`WriteView`，经 `#[repr(C)]` shim 表 `PweCtx` 回调）均生成 C、用系统 `cc -O2 -ffp-contract=off` 编译为 `.dylib`/`.so`、`dlopen` 加载。差分测试：纯函数（含除零 trap/`signum`/位精确条件的输入矩阵）、nbody 写入逐位一致；基准 nbody-64 步进 1323µs vs 解释器 2355µs（~1.8×）。依赖仅系统 C 编译器（缺失返回错误）。
- [x] 契约：#35 —— `NativeProgram` 先 `validate`+`verify_linear_dominance`（从不跳过 Validate）、按内容哈希标识产物（RFC-0035 风格，同时修 #31）、0700 唯一临时目录并在 Drop 清理；**RFC-0027 增补条款**定义了「进程内原生内核」窄豁免（CapabilityCheck/Publish 跳过、其余照旧）。**该后端尚未接入 `pwe run`/`present` 等生产入口**——接入前必须实现完整生命周期（manifest/CapabilityCheck/Publish）。
- [x] 世界访问型原生内核：`ReadView`/`ReadCommitted`/`WriteView` 经 `#[repr(C)]` 函数指针表（`PweCtx`）回调进 `NativeCtx{rt,writes}`；`execute_entries` 复刻解释器的系统屏障/顺序语义。差分测试 `native_world_kernel_matches_interpreter_writes` 通过（nbody 写入逐位一致）。基准：nbody-64 步进 1323µs vs 解释器 2355µs（~1.8×）。
- [x] GPU/NPU 计算后端（WGSL，`wgsl.rs` + `WGSL_TARGET`）：把数据并行的 map 核（无世界访问/无调用/无分支的直线函数）下降为 WGSL 计算着色器（`@compute` / `@workgroup_size` / storage buffers）。**设备语义为 f32**（WebGPU 无 f64），故为**近似**设备后端、非逐位等价。验证：依赖无关的结构校验器 + f32 CPU oracle 与解释器在 f32 容差内一致（`wgsl::tests`）。
- [ ] WGSL 执行验证：本机浏览器无 WebGPU 宿主（`navigator.gpu` 缺失），无法执行着色器；需 WebGPU 宿主或以 `naga`（dev-only，受 MSRV 约束）做离线编译校验后再接入运行。

### Phase 4 — 普适性与生态
- [ ] 语义化模块系统；fmt/REPL/LSP；doctest
- [ ] 分布式/插件沙箱、fuzz/Miri
- [ ] EIR 升级为显式 SSA CFG；RFC↔conformance ≥80%
- [ ] ADR/贡献指南/架构文档

## 4. 已完成（本次迭代）

1. **系统内同步屏障**：新增 `EIR_EFFECT_BARRIER` 每个系统首函数标记，`execute` 在系统边界调用
   `commit_barrier`；`SceneRuntime` 增加 `committed` 快照。跨实体读改为 `ReadCommitted`（start-of-system），
   系统内所有实体读到同一冻结快照，跨系统可见先前写。**修复了 nbody 破坏牛顿第三定律的根因**，并新增两个回归测试。
2. **太阳系演示科学修正**：真实轨道半径比例 + 真实质量 + **质心系初值**，月球真正绕地球（希尔半径 ~0.26 倍）。
3. **工程基座**：CI、工具链、`cargo-deny`、`.cargo` 策略、无依赖基准。
4. **性能度量**：基准暴露 `compile` O(n²) 与 `validate` 常数过高；已优化至 38ms。

## 4b. Phase 3 进展（本次迭代）
- 解释器读路径 + 调用索引优化；优化层的常量折叠与 FMA 融合（`Fma` opcode）。
- 真原生后端（纯函数）：C 代码生成 + `cc` + `dlopen`，与解释器逐位一致（差分测试）。
- 基准（release，本机）：`step_interpreter(nbody 64)` ≈ 2.21 ms（优化前 2.28），`step_cross` ≈ 4.5 ms，`compile(nbody 64)` ≈ 93 ms（优化在编译期，代价 +~24%）。
- 差分验证：全部 `step_cross`/conformance 用例通过（优化模块 ≡ 通用模块，字节一致）。

## 5. 验收标准（关键项）
- 运行路径 `unwrap=0`；CI 全绿；基准回归 >5% 报警。
- 多体/群集任意系统由构造保证同步（conformance）。
- `compile(nbody 64)` 显著下降（目标 <100 ms）。
- 解释器 ≥3×；真实 JIT 热点 ≥10×；GPU 场结果与 CPU 容差内一致。
