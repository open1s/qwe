# PWE 全面评审与改进路线图

> 评审视角：编程语言设计 + 系统架构 + 计算科学 + 性能工程。
> 依据：仓库实测（LOC、依赖、语法、RFC、测试、clippy/fmt/conformance、基准）。

## 0. 总评

PWE 方向正确：**微内核 + 分层 IR（WIR → Domain IR → EIR）+ 确定性优先 + 冻结 ABI + 能力模型**，
已能表达 4D 物理/化学/生物仿真，并有可用的实时 3D 查看器。当前处于“**广度高、深度浅**”阶段：
模块已拆分、原生 JIT/AOT 与 CI/基准门禁已交付、非测试 `unwrap` 归零；**深度缺口**集中在
**类型系统（仍仅 f64）**、**GPU 尚非加速器**、**分布式仅进程内回环**、**插件宿主未实现**，
语义带 v0.3 pragma 但尚未冻结。

定位：**优秀的“可编程物理世界”确定性与原型平台，尚未成为生产级通用科学计算语言。**

## 1. 现状度量

| 指标 | 值 |
| --- | --- |
| Rust 总 LOC | ~49.4k（reference 生产 41.0k、reference 测试/示例/基准 4.6k、cli 1.6k、conformance 1.2k、`pwe-api` 1.0k） |
| 最大文件 | `lang/tests.rs` 4.3k；`eir.rs` 4.3k；`lang/systems.rs` 3.5k（原 `lang.rs` 10.8k 已拆分为 8 个模块） |
| 外部依赖 | 运行时仅 `pest`（+ macOS 可选 `metal`）；dev 依赖仅 `naga`；无其它重依赖 |
| `pwe-api` | `#![no_std]`、零依赖、82 个 `pub` 项、`include/pwe_abi.h`（布局被测试锁定） |
| RFC | 47 份（RFC-0001…0048，其中 0043…0048 为扩展集）；conformance 28 用例 |
| 测试 | 439 个（单元 381 + laws 21 + property 4 + stdlib 6 + fmt 1 + fuzz 2 + `pwe-api` 15 + cli 9） |
| 非测试 `unwrap()` | 0（`clippy.toml` + `#![cfg_attr(not(test), deny(clippy::unwrap_used))]` 防回归） |
| `unsafe` | 31 处，集中在 `src/ffi.rs`(10)/`native.rs`(19)/`gpu.rs`(2) |
| JIT/AOT | **原生码已交付**：C 代码生成 + 系统 `cc` + `dlopen`，与解释器逐位差分；nbody-64 步进 1323µs vs 解释器 2355µs |
| CI/基准 | `gate.sh`（fmt/clippy/test/conformance/examples）+ CI（gate/supply-chain/commit-lint）+ 基准回归门禁 `tools/bench-check.sh` |

## 2. 分维度评估

### 2.1 代码实现与架构
- 优点：依赖极简、契约分层清晰、`step_cross` 逐字节差分、ordinal 稳定。
- 现状：`lang.rs` 单体已拆（8 模块）；**EIR 指令元数据已单源**（`declare_opcodes!` 生成
  enum/`from_u16`/`result_type`，全 opcode round-trip 测试）；非测试 `unwrap=0`；CI/工具链
  固定/`cargo-deny` 已就位。残留：`eir.rs`(4.3k)、`lang/systems.rs`(3.5k) 仍偏大。

### 2.2 语法与语言设计
- 优点：领域关键词清晰；单位词法无歧义；`1/60` 分数；`vecN`/`struct` 铺平；`=`/`inte`/`deriv(E)` 语义已自洽。
- 问题：`funcs` **位置式 `s0..` 形参**；表达式靠 PEG 顺序消歧、脆弱；**仅 f64 用户可见**
  （`let` 标注、`i64/f64/bool(…)` 转换、int/bool 直达寄存器（RFC-0043）与具名数组（RFC-0044）均已交付）；`import` 仅相对路径、无命名空间；语义带 v0.3 pragma + 迁移器但**尚未冻结**。

### 2.3 科学正确性
- 优点：`tests/laws.rs` 21 项解析解校验；std 覆盖 12 域；系统内同步屏障已交付。
- 问题：`invariant`/`conserved{tolerance}` 运行时断言已实现但**示例与 CI 未接入**；
  CFL 已计算（`velocity*dt/dx`）却无编译期检查；diffuse `rate ≤ 1/4` 稳定性只写在注释里。

### 2.4 功能完备性
- 已具备：解释器/原生 JIT/AOT/线程化、跨后端差分、EIR 二进制/快照、分布式所有权（进程内）、能力、
  池化、关节、软体、结构体、PDE 场、通道、模块系统（RFC-0045）、SSA CFG 表示（RFC-0046）、
  fmt/REPL/LSP/playground/doctest、查看器。
- 缺口：GPU 加速（#41 设备常驻）；QUIC 传输与插件宿主（类型系统 int/bool/数组已由 RFC-0043/0044 交付）
  （P4）；`funcs` 具名形参。

### 2.5 易用性
- 三命令 CLI、双语教程文档、带定位诊断、`pwe fmt`/`repl`/`lsp`/`doctest`/`playground`。
- 缺：`funcs` 位置参数；相对导入/命名空间；物理示例无运行时断言。

### 2.6 稳定性与安全
- 非测试 `unwrap=0`（clippy 防回归）；`unsafe` 31 处集中在 `ffi`/`native`/`gpu` 且有安全契约注释；
  fuzz + Miri 用法已入 CONTRIBUTING；确定性策略成文（RFC-0013）且有 conformance 覆盖。

### 2.7 性能（已量测，含回归门禁）
- 既有优化：批量场 opcode、稠密覆盖层、`step_cross_batched`、读路径缓存、常量折叠 + `Fma` 融合。
- 曾定位并修复：`compile(nbody 64)` 的 `validate_function` O(F) 查找与 `BTreeMap` SSA 表常数
  （96→38ms）。
- 基准回归门禁：`tools/bench-check.sh`（本地对 `reference/benches/baseline.txt` 5% 阈值；
  CI 走硬顶），接入 `gate.sh` 与 `ci.yml`。

### 2.8 可扩展性
- EIR 已有显式 SSA CFG 表示与 wire 形式（RFC-0046）；opcode 元数据单源；RFC↔conformance 25/0。
- 残留：块参数 SSA 与跨块优化（DCE/GVN）；RFC-0043/0045 已补齐（0044 仅剩运行期下标越界检查）。

## 3. 路线图

### Phase 0 — 工程基座（进行中）
- [x] CI（fmt/clippy `-D warnings`/test/conformance/examples/bench）+ `rust-toolchain.toml` + `deny.toml` + `.cargo/config.toml`
- [x] 手写基准 `reference/benches/throughput.rs`（无重依赖）
- [x] 消除非测试 `unwrap()`（134→0；固定长转换用 `.expect("proven")`，解析用 `next_pair` 传播，锁用 `unwrap_or_else(into_inner)`）；以 `clippy.toml` + `#![cfg_attr(not(test), deny(clippy::unwrap_used))]` 防回归；FFI 类型别名补安全契约说明
- [x] **系统内同步屏障（引擎通用语义）**：核心已交付（见 Phase 2「系统内同步屏障」）
- [x] 编译期性能：`validate_function` 去 O(F) 查找、SSA 表改稠密数组（compile(nbody 64) 96→38ms）

### Phase 1 — 语言与运行时定型/拆解
- [x] 拆分 `lang.rs`（11077→mod.rs 387）：`systems.rs`/`tests.rs`/`compile.rs`/`lower.rs`/`parser.rs`/`runtime.rs`/`ast.rs`/`diagnostics.rs`
- [x] opcode 元数据单一事实源 + 全 opcode round-trip 测试（`declare_opcodes!` 单源生成
  enum/`from_u16`/`result_type`；`from_u16(op as u16) == Some(op)` 全表断言）
- [x] 语言版本 pragma（`world { lang_version = "0.3" }`，detail 83）**+ 迁移器**：`pwe migrate` / `lang::migrate_v02_to_v03`（旧隐式 `=` 积分与旧 `deriv` → `inte`）
- [ ] `funcs` 具名形参

### Phase 2 — 类型系统与科学正确性
- [x] 系统内同步屏障（`Barrier` = 系统边界；committed = start-of-system）
- [x] 值类型（**RFC-0043 Done**）：`let` 类型标注（89）、整型常量语义、显式转换 `i64/f64/bool(…)`；**int/bool 直达 EIR 寄存器**（`I64ToF64`/`F64ToI64`，精确整数运算，解释器为语义基准），conformance "RFC-0043 typed int/bool values (exact integer arithmetic, cross-backend)" + `lang::tests::int_let_uses_integer_semantics`/`rfc_0043_integer_and_fractional_literals_mix`
- [x] **RFC-0044 具名定长数组**（`array N name`、`name[j]` 读/写/`+=`/`inte`、常量下标越界 detail 52、未知数组 detail 109、内联初始化、**`for j in 0..len(name)` 编译期长度边界**）；`vecN`/`s[i]` 不变；conformance + 单测（运行期下标越界检查列为 Deferred）
- [~] 物理合理性运行时：`invariant`/`conserved{tolerance}` 运行时断言与守恒/有界回归测试已交付；**CFL 与 diffuse 稳定性仍是「算出但不检查」，示例未接入**（见 2.3）
- [ ] 物理 demo 断言纳入 CI

### Phase 3 — 高性能与多后端
- [x] 解释器读路径优化：缓存规范组件 id（免 OnceLock 原子）、`pending`/`committed` 空覆盖快路径、覆盖表 BTreeMap→HashMap；缓存调用索引（`CallIndex`，免每步排序+哈希）。既有形态本已是"预译码"（扁平 `Vec<Instruction>` + 稠密寄存器 + 跳表分派）。
- [x] 优化执行层（AOT/JIT 共用）：`EirModule::optimize` 做**常量折叠 + `Mul/Add→Fma` 超指令融合**（新增 opcode `Fma=235`，语义 = `(a*b)+c` 两次舍入，**位等价**）。`AotProgram::compile` 于编译期优化；`LangRuntime` 用优化模块执行、`jit` 保持通用 → `step_cross` 差分验证优化器。nbody-64 步进 ~7%；`Fma` 融合使算术指令 2048→784。
- [x] 真原生 AOT/JIT（`native.rs` + `NATIVE_TARGET`）：纯 `funcs`（无世界访问）与**世界访问型内核**（`ReadView`/`ReadCommitted`/`WriteView`，经 `#[repr(C)]` shim 表 `PweCtx` 回调）均生成 C、用系统 `cc -O2 -ffp-contract=off` 编译为 `.dylib`/`.so`、`dlopen` 加载。差分测试：纯函数（含除零 trap/`signum`/位精确条件的输入矩阵）、nbody 写入逐位一致；基准 nbody-64 步进 1323µs vs 解释器 2355µs（~1.8×）。依赖仅系统 C 编译器（缺失返回错误）。
- [x] 契约：#35 —— `NativeProgram` 先 `validate`+`verify_linear_dominance`（从不跳过 Validate）、按内容哈希标识产物（RFC-0035 风格，同时修 #31）、0700 唯一临时目录并在 Drop 清理；**RFC-0027 增补条款**定义了「进程内原生内核」窄豁免（CapabilityCheck/Publish 跳过、其余照旧）。**生产路径经 JIT 生命周期门**（Validate/CapabilityCheck/Publish）：hotness 提升在 `ready()` 之后发生，`pwe run` 默认启用原生 JIT（`--no-native-jit` 关闭）；独立 `NativeProgram` API 仍属豁免。
- [x] 世界访问型原生内核：`ReadView`/`ReadCommitted`/`WriteView` 经 `#[repr(C)]` 函数指针表（`PweCtx`）回调进 `NativeCtx{rt,writes}`；`execute_entries` 复刻解释器的系统屏障/顺序语义。差分测试 `native_world_kernel_matches_interpreter_writes` 通过（nbody 写入逐位一致）。基准：nbody-64 步进 1323µs vs 解释器 2355µs（~1.8×）。
- [x] GPU/NPU 计算后端（WGSL，`wgsl.rs` + `WGSL_TARGET`）：把数据并行的 map 核（无世界访问/无调用/无分支的直线函数）下降为 WGSL 计算着色器（`@compute` / `@workgroup_size` / storage buffers）。**设备语义为 f32**（WebGPU 无 f64），故为**近似**设备后端、非逐位等价。验证：依赖无关的结构校验器 + f32 CPU oracle 与解释器在 f32 容差内一致（`wgsl::tests`）。
- [x] WGSL **离线校验**：`naga`（dev-dependency）对每个产出的着色器做 parse+validate；`sign`/`round`/`hypot`/`Rem`/`Select` 以显式 helper 复刻 CPU 语义（#37/#38），oracle 与之镜像，边界矩阵覆盖 ±0/±inf/NaN/subnormal/半值/大值。RFC-0021 trap（除零/NaN 比较）以 `atomic<u32>` **trap 标志 + 提前返回**在设备上复现（#39），宿主读标志并以 detail 18 失败该步。
- [~] **GPU f32 卸载（Metal on Mac，`--features gpu`，实验性）**：`gpu.rs` 用 Metal 计算着色器（MSL）在 GPU 上跑**场 stencil（diffuse）**；`pwe run --gpu` 启用（`SceneRuntime::field_diffuse` 走 GPU，其余回落 CPU）；`metal` 按 `target_os="macos"` 门控，非 macOS/CI 不构建。**f32 近似**（CPU f64 为基准）。**实测慢于 CPU**（#41：256² 上 CPU 129µs vs GPU 244µs；各尺寸 1.34×–2.55× 慢）——原因是每步 `f64↔f32` 主机转换 + 上传/回读 + 同步，而非算法。**属于正确性已验证的卸载路径，非加速器**；要真正提速需设备常驻场状态（修 #41，未实现）。基准记录见 `benches/throughput.rs::bench_gpu`。
- [x] WGSL **执行验证（Metal on Mac）**：`gpu-verify/`（独立 crate，自带 `[workspace]`，CI 不受影响）用 `wgpu` 的 **Metal** 后端在真实 GPU（Apple M2 Max）上执行产出的 WGSL，与 CPU oracle/解释器在 f32 容差内一致（64 lane），并验证除零 **trap 标志**置位。运行：`cargo run --release --manifest-path gpu-verify/Cargo.toml`。
- [x] 真 JIT（hotness→原生→deopt）：`CpuJit` 在模组达 `NATIVE_PROMOTE`(=64) 次调用后把单元提升为**原生代码**执行（`native_for` 懒编译并缓存失败以 deopt 回解释器）；提升发生在 JIT 生命周期门（Validate/CapabilityCheck/Publish，`ready()`）**之后**，故合规。`LangRuntime::enable_native_jit` / `pwe run --native-jit` 启用；差分测试 `native_jit_promotes_and_matches_interpreter` 断言提升后确有原生执行（`native_executions>0`）且 `step_cross` 逐步字节一致。
- [x] 线程化分派（threaded dispatch）：**已作为 opt-in 后端采纳**（`--threaded` / `LangRuntime::enable_threaded_dispatch`）：`eir.rs` 新增静态函数指针表（按 opcode 索引）的线程化解释器，覆盖纯算术/比较/数学/`ReadView`/`WriteView`/调用/分支等子集，整模块不支持则回退跳表；差分测试 `threaded_dispatch_matches_jump_table` 通过（与跳表逐位一致，且经 `step_cross` 对 JIT 验证）。**默认仍为跳表**（线程化端到端 nbody-64 3086µs vs 跳表 2264µs，慢 ~36%；微基准 8%）。基准记录见 `benches/throughput.rs::step_threaded`。
- [x] SIMD：**评测后不采用**——场 stencil 内点循环为无分支单位步长，LLVM 已自动向量化；显式 2-lane（SSE2/NEON）实测**更慢**（256 宽行 0.128µs vs 标量 0.096µs），故保留标量（由 LLVM 向量化）并记录测量。

### Phase 4 — 普适性与生态
- [x] 语义化模块系统；fmt/REPL/LSP；doctest
  - [x] **playground**：`pwe playground [--port P]` —— 本地浏览器编辑器 + 实时 3D 视图（`present.rs::serve_playground`；驱动线程独占 runtime，经通道接收源码，复用 `present` 的 `/view` viewer 与 `/state`）。`POST /api/source` 编译并返回诊断，成功则实时步进渲染。含 `playground_page_has_editor_and_viewer` 测试；端到端手工验证（编译样例 + `/state` 帧 + 错误诊断）。
  - [x] **`pwe fmt`**：token-preserving 重格式化（2 空格缩进 —— 与仓库一致，`reference/tests/fmt.rs` 逐个校验已提交 `.pwe` 可过 `--check`；去尾空白、折叠空行；`--check`/`-w`）。保证**语义不变**：仅改前导空白/行尾/空行，EIR 逐字节一致（测试 `format_is_idempotent`/`format_preserves_tokens`/`formatting_preserves_compiled_artifact`）。
  - [x] **`pwe repl`**：交互式输入源码并 `:run [N]`/`:step [N]`/`:reset`/`:show`/`:clear`/`:load`/`:quit`；核心 `run_repl<R:BufRead,W:Write>` 可脚本化并单测（`repl_script_compiles_runs_and_steps`、`repl_reports_diagnostics`）。
  - [x] **doctest**：`pwe doctest [FILES...]` 编译 Markdown 中可运行的 ```` ```pwe ```` 完整程序块（以 `world` 开头；`pwe ignore` 标记示例片段跳过）；`doctest.rs` 含抽取/选择/文档回归测试（`shipped_docs_compile` 校验 README/lang-usage en+zh）。
  - [x] **LSP**：`pwe lsp`（stdio，无依赖）——全文档同步、`publishDiagnostics`（开/改/关，`lang::compile`+`diagnose`）、`textDocument/formatting`（`format_source`）；自带 JSON 解析/序列化（`json.rs`）与单测。
  - [x] 语义化模块系统（**RFC-0045**：稳定模块名、显式 export/私有、确定性合并与冲突报错、模块集纳入 artifact 身份）——conformance "RFC-0045 semantic modules (import + export, cross-backend)"、"RFC-0045 module export surface enforced (detail 102)"，`lang::tests::module_*`、`from_import_respects_export_surface`
- [~] 分布式/插件沙箱、fuzz/Miri
  - [x] **fuzz（依赖无关、CI 可跑）**：`reference/tests/fuzz.rs` 用确定性 PRNG 向所有公开解码边界（EIR/extension/channel）与 parser/compiler/formatter 灌入随机字节/源码，断言**不 panic**、解析返回 `Result`、`format_source` **幂等**。Miri 用法记于 CONTRIBUTING（`cargo +nightly miri test --test fuzz`；`cfg!(miri)` 下自动减迭代）。
  - [ ] 分布式/插件沙箱（更大）——**已排期 P4**：RFC-0006 QUIC 传输 + RFC-0016 同进程能力级插件宿主
- [~] EIR 升级为显式 SSA CFG；RFC↔conformance ≥80%
  - [x] RFC↔conformance：冻结集 RFC-0019–0036 全覆盖；**扩展 RFC-0037–0042 各有 conformance 用例**（场扫描/池/关节/软体/struct，cross-backend），**RFC-0043/0045 再增 3 例**，**RFC-0048 Slice A（零穿越检测 cross/rise/fall/last_cross）再增 1 例**，报告 **total=28 failed=0**；`docs/rfc-alignment.md` 增补扩展 RFC 表与 general-simulation 小节（0043/0044/0045/0046 均为 Done；0048 Slice A Partial）。
  - [x] EIR 显式 SSA CFG（RFC-0046，已实现表示层+wire）：`EirModule::blocks`/`verify_cfg`；`verify_linear_dominance` 改用具实块；FUNCTIONS 段对含分支函数编码显式块（`block_count>1`，兼容旧单块），解码确定性拼接、重编码逐字节一致（`eir_cfg_blocks_round_trip`）。残留：块参数 SSA 与跨块优化（DCE/GVN）。
- [x] ADR/贡献指南/架构文档：`docs/architecture.md`（流水线/边界/执行层）、`CONTRIBUTING.md`（工具链/门禁/特性/流程）、`docs/adr/`（0001–0005：原生 cc 后端、进程内豁免、opt-in 线程化、SIMD 不采用、GPU 实验性）。

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
  - #91：原生编译禁用编译器内建替换（`-fno-builtin`）。clang 会把成对出现的
    `sin(x)`/`cos(x)` 融合为 `__sincos_stret`，其正弦与 libm `sin`（解释器
    `f64::sin` 所调用的实现）相差 1 ULP；热升级后 `step_cross` 逐位比较即失败。
    同时把 `present` 的交叉校验相位对齐到 `run`（两者同为每 16 步的最后一步）。
- 基准（release，本机，权威记录见 `reference/benches/baseline.txt`，由
  `tools/bench-check.sh --update` 生成）：`compile(nbody 64)` ≈ 93.9 ms、
  `step_interpreter(nbody 64)` ≈ 2.36 ms、`step_cross` ≈ 5.09 ms、
  `step_native(nbody 64)` ≈ 1.36 ms、`step_threaded(nbody 64)` ≈ 3.24 ms、
  `step_interpreter(wave 32x32)` ≈ 8.6 µs、`present_frame(nbody 64)` ≈ 7.1 µs。
- 差分验证：全部 `step_cross`/conformance 用例通过（优化模块 ≡ 通用模块，字节一致）。

## 5. 验收标准（关键项）
- 运行路径 `unwrap=0`；CI 全绿；**基准回归 >5% 报警已接入**（`tools/bench-check.sh`，本地对
  `reference/benches/baseline.txt` 比对、CI 走硬顶；`gate.sh` 与 `ci.yml` 均调用）。
- 多体/群集任意系统由构造保证同步（conformance）。
- `compile(nbody 64)` 显著下降（目标 <100 ms）。
- 解释器 ≥3×；真实 JIT 热点 ≥10×；GPU 场结果与 CPU 容差内一致。
