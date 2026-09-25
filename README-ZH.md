# PWE —— 用 4D 编写物理世界。

**一个 Rust 微内核运行时 + 语言，用于确定性的、分布式的 4D 世界仿真——3D 空间，加上时间。**

[English](README.md)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.81%2B-orange.svg)](https://www.rust-lang.org)
[![tests](https://img.shields.io/badge/tests-304%20passing-brightgreen.svg)](#测试与符合性)
[![conformance](https://img.shields.io/badge/conformance-17%2F17%20%C2%B7%200%20skips-brightgreen.svg)](#测试与符合性)
[![RFCs](https://img.shields.io/badge/frozen%20contract-37%20RFCs-purple.svg)](#冻结契约)
[![repo](https://img.shields.io/badge/github-open1s%2Fqwe-181717.svg)](https://github.com/open1s/qwe)

PWE 不是又一个物理玩具，而是一个**基底**：唯一权威的世界、统一的类型化中间表示、
一种语言——足以建模*任何*可写成耦合微分/差分方程的规律：重力与碰撞、化学动力学、
种群演化、电磁、热、声、波、机械臂，以及会走路的机器。

而且它的确定性**可被证明**：解释器是语义基准，JIT 必须与它**每一步逐字节一致**；
300+ 测试与 17 项符合性检查强制保证——**零跳过**。

```
World Model → WIR → Domain IR → EIR → 解释器 / JIT / AOT → Runtime → CPU / GPU / NPU / Edge / Cloud
```

---

## 60 秒上手

```sh
cargo build --release -p pwe-cli

# 一个类人形象在行走（62 部件：胶囊四肢、眼睛、手指）
./target/release/pwe compile cli/examples/humanoid.pwe -o humanoid.pweb
./target/release/pwe present humanoid.pweb --port 8000   # 打开 http://localhost:8000

# 脉冲在三维立方体中辐射——先 ⟳ Restart，再 ▶ Resume
./target/release/pwe compile cli/examples/wave3d.pwe -o wave3d.pweb
./target/release/pwe present wave3d.pweb --port 8000

# 一整个太阳系，实时
./target/release/pwe compile cli/examples/solar.pwe -o solar.pweb
./target/release/pwe present solar.pweb --port 8000
```

每个产物都是**自描述、已校验的二进制**（`.pweb`）：重新编译、重新运行、重新发布——
同样的字节，同样的世界。

---

## PWE 的不同之处

| 常见引擎 | PWE |
| --- | --- |
| 一套固定的内置行为，供你配置 | **一种语言**——把任何规律写成状态上的规则 |
| "差不多确定性"，靠约定信任 | **可证明的确定性**——解释器 ≡ JIT，每一步断言 |
| 物理*或*化学*或*渲染*，分散在不同工具 | **同一世界上的对等域**——物理、渲染、感知、AI、网络共享唯一事实源 |
| 2D/3D 场景 | **4D 基底**——3D 空间 + 时间；网格场是体积的 |
| 一个你祈祷它稳定的黑盒 | **冻结、版本化的契约**——37 份 RFC、规范字节、内容哈希 |
| 封闭的可视化 | **开放的实时查看器**——3D 网页视口，Restart / Pause / Labels |

**世界是唯一权威的事实源。** 实体是身份，组件是数据，系统是行为；一切权威变更都
经过 `WorldTransaction`。

---

## 一种语言，覆盖所有领域

PWE 的语言在所有学科里都是同一套——只有方程在变。

```pwe
world {
  gravity = (0, 0, 0)
  field heat { width = 32; height = 32; depth = 32; dx = 1.0 }   # 一个 3D 网格场
  entity body { state = (x = 1.0, vx = 0.0, temp = 353.15) }
}
systems {
  diffuse { field = heat; rate = 0.16 }        # PDE：T += rate·∇²T
  update  { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.radiative_cooling(temp, 293.15, 0.9, 1.0, 1.0, 700.0) + 0.0
  }
}
```

| 领域 | 在 PWE 中 |
| --- | --- |
| **力学 / 重力 / 碰撞** | `nbody`、`gravity`、`integrate`、`ground_contact`、`wall`、`force`、`linear`、`update` / `rk4` |
| **PDE 场（4D）** | `field` + `diffuse` / `poisson` / `wave`（热、电势、声、波） |
| **化学** | `std/chemistry`——完整元素周期表（Z = 1–118）、Arrhenius、动力学、pH |
| **热力学** | `std/thermal`——牛顿冷却、Stefan–Boltzmann、热传导 |
| **声学与光学** | `std/acoustics`、`std/optics`——声压级、多普勒、Beer–Lambert、菲涅尔 |
| **电磁** | `std/em` + `poisson`——库仑、洛伦兹、回旋、坡印廷 |
| **机器人** | `std/robotics`——正/逆运动学、PID、差速驱动 |
| **任何随机过程** | `random()`、`noise()`、`emit()`——有种子、可复放 |

**标准库**本身就是 PWE：`std/` 提供 `math`、`particles`、`forces`、`mechanics`、
`chemistry`（1–118）、`thermal`、`acoustics`、`optics`、`em`、`robotics`、`units`、
`control`——纯函数 + 具名命名空间的物理常量，可用 `--param` 覆盖。

---

## 展示 —— 让它动起来

每个都用 `pwe compile … -o out.pweb` 编译，再 `pwe present out.pweb`。

| 演示 | 是什么 |
| --- | --- |
| `humanoid.pwe` | 一个**62 部件的类人形象**在行走——胶囊四肢、面部细节、带指节的手指。 |
| `robot.pwe` | 一条由 `std/robotics` 正运动学驱动的**2 连杆机械臂**在摆动。 |
| `shapes.pwe` | **自定义形状**：组合体、凸八面体、显式面宝石、挤出的 **SVG** 星形。 |
| `wave3d.pwe` | 在立方体中辐射的 **3D 波**——半透明、按能量着色的球面壳向外扩散并衰减。 |
| `acoustics.pwe` | 驱动单极子产生的 **2D 声场**；探针记录到达延迟与 dB 电平。 |
| `solar.pwe` | **太阳系**实时：8 大行星 + 月球，公转兼自转。 |
| `flock.pwe` | 通过邻域查询**聚合并对齐**的鸟群。 |
| `domains.pwe` | 阻尼弹簧上的探针同时**辐射降温**——力 + 热 + 电磁 + 化学的组合。 |
| `spring/spring.pwe` | 多文件**模块**、参数、单位与计划事件（`--param k=16`）。 |
| `wave.pwe` | 一维线上的**正弦驻波**，画成按能量着色的曲线。 |
| `heat.pwe` | 二维网格上的**热扩散**（展开的 Gauss–Seidel 扫描）。 |
| `bounce.pwe` | 经典：**重力 + 地面接触**与弹性。 |

查看器按实体自身的声明渲染——`shape`、`size`、`color`、`opacity`、`glow`、
`label`；场渲染为等值面（2D/3D）或曲线（1D）。控件：**⟳ Restart**、
**⏸ Pause / ▶ Resume**、**🏷 Labels**。

---

## 语言一瞥

```pwe
world {
  gravity = (0, -9.81, 0)
  entity vehicle {
    position = (0, 8, 0)  velocity = (4, 0, 0)
    mass = 4  dynamic = true  box = (1, 0.5, 0.7)
  }
  entity ground { position = (0, -5, 0)  dynamic = false  box = (50, 5, 50) }
}
systems {
  gravity { gravity_y = -9.81; dt = 1/60 }
  integrate { dt = 1/60 }
  ground_contact { restitution = 0.6 }
}
```

* **规则即方程**：`slot = expr` 表示 `slot += dt·expr`（欧拉）；`rk4` 对相同规则做
  4 阶积分。
* **是状态，不是脚本**：命名槽、跨实体读取（`@other.state.x`）、属性
  （`@other.mass`）、空间查询（`neighbor_count`、`nearest_dist`、`neighbor_mean`）。
* **Python 式模块**：`import "std/forces"`、`import "m" as x`、
  `from "m" import f`；包即目录；循环导入可解析。
* **单位，可检查**：可选的 `[m/s^2]` 标注 + 渐进式量纲分析——未标注即通配符。
* **事件与调度**：`at(T)`、`periodic(P)`、`schedule(gate, delay, …)`、
  `emit` / `last_event`——在步网格上精确一次、确定性。
* **有界控制流**：`repeat` / `for` / `break` / `continue` 展开为直线式 EIR；
  一个步内做牛顿迭代只需一行。
* **会教学的诊断**：编译失败打印 detail code 与出错源码行（javac 式脱字符）。

完整参考——词法、**全部关键字**、EBNF、运算符优先级，以及**内建函数完整表**——
见 [`docs/lang-usage.zh.md`](docs/lang-usage.zh.md)。

---

## 可证明的确定性

这里的确定性不是口号，而是**编译与运行的闸门**：

* **解释器 ≡ JIT**：`step_cross` 在相同状态上跑两个后端，要求每一步的写入*以及*
  事件/队列流都逐字节一致。
* **解析解符合性**：每个仿真都对照其闭式解断言（牛顿冷却、放射性衰变、
  logistic 增长、开普勒轨道、简谐能量、作用-反作用、可逆动力学、墙面反射……）。
* **快照 / 重放**：状态哈希在多次运行与快照之间逐位复现。
* **冻结契约**：37 份 RFC 定义规范字节、模式与协议；符合性**零跳过**。

---

## 架构

```
application  →  world  →  IR  →  compiler  →  runtime  →  kernel  →  platform
```

| Crate | 职责 |
| --- | --- |
| `pwe-api` | `no_std`、零依赖的**冻结契约**（类型、trait、ABI、限制、detail code）。 |
| `reference/`（`pwe-reference`） | **语义基准**：确定性世界、解释器、解释器支持的 JIT、AOT、语言前端。 |
| `conformance/` | RFC-0029 报告运行器 + RFC-0030 场景。 |
| `cli/`（`pwe`） | 工具链：`compile` / `run` / `present`。 |

`pwe-reference` 内部：

| 模块 | 用途 |
| --- | --- |
| `lang` | **PWE 语言**：PEST 语法 → EIR、跨后端执行、诊断。 |
| `physics` / `physics_eir` | 确定性刚体仿真；可复用的 `EirSystem` 降级为类型化 EIR。 |
| `field` / `continuum` | 通用 4D 标量场：存储、拉普拉斯、Poisson、扩散、内容哈希。 |
| `present` | 3D 网页呈现层：帧 → JSON → 实时 Three.js 查看器。 |
| `eir` / `dominance` / `fence` / `aot` | 类型化 SSA IR、支配性校验、显式栅栏、AOT 工件编解码。 |
| `broadphase` / `cluster` / `channel` / `distributed` | 可互换宽相；类 BEAM 集群；跨运行时通道；分布式连续场。 |
| `simulation` | `Input → Physics → Commit → RenderPrepare`、跟随相机、带哈希快照。 |

---

## 快速开始

```sh
git clone git@github.com:open1s/qwe.git && cd qwe

cargo build --workspace
cargo test  --workspace          # 304 个测试
cargo run -p pwe-conformance     # RFC-0029：全部 PASS，无跳过

# 语言端到端：
cargo run -p pwe-reference --example language_demo
```

**环境要求：** Rust 1.81+。`pwe-api` 为 `no_std` 且零依赖；`pwe-reference` 仅依赖
`pwe-api` 与 PEST。

---

## 测试与符合性

| 套件 | 数量 |
| --- | --- |
| 运行时 / 语言单元测试 | 266 |
| 解析解符合性 | 17 |
| 属性测试 | 3 |
| 标准库测试 | 3 |
| 集成 / 其它 | 15 |
| **合计** | **304** |

外加 `pwe-conformance`：**17 / 17，零跳过**。`no_std` 检查：
`cargo check -p pwe-api --no-default-features`。

---

## 冻结契约

完整的冻结 v0.2 契约（RFC-0019 … RFC-0036）：模式与规范化、WIR、EIR（类型化 SSA +
支配性）、内存与栅栏、世界事务、分布式所有权、快照与增量、运行时 ABI
（`include/pwe_abi.h`）、JIT/AOT、渲染帧、符合性与最小剖面、组件 ABI、Domain IR、
能力、错误/限制、工件身份、交换信封。见 [`rfc/`](rfc) 与
[`docs/rfc-alignment.md`](docs/rfc-alignment.md)。

---

## 诚实的边界与路线图

内核边界之后有界、刻意的缺口——**不是**缺失的契约：

* **后端**：参考实现提供解释器（基准）、解释器支持的 JIT 与 AOT 路径——全部做过
  差分验证。GPU / NPU / SIMD 在 `AotProgram.target` 边界接入，**在路线图上**。
  参考"JIT"锁定的是 JIT *契约*，尚未生成原生机器码。
* **刚体**：AABB、球、凸包（精确 SAT）、复合与高度场碰撞体，冲量、摩擦、距离关节、
  地面。尚无软体或完整关节族。
* **动态实体集合在编译期固定**（对象池激活已列入计划）。

实时清单见 [`tasks/todo.md`](tasks/todo.md)。

---

## 仓库结构

| 路径 | 内容 |
| --- | --- |
| `pwe-api` | 冻结的 `no_std` 契约。 |
| `reference/` | 语义基准 + JIT/AOT + PWE 语言。 |
| `conformance/` | 符合性运行器。 |
| `cli/` + `cli/examples/` | `pwe` 工具链与可直接运行的世界。 |
| `std/` | 标准库（纯函数模块）。 |
| `rfc/` · `docs/` | 冻结契约与使用指南。 |
| `include/pwe_abi.h` | 生成的 C11 ABI 头，与布局锁定。 |

---

## 许可证

Apache-2.0。Copyright 2026 Open1s。见 [LICENSE](LICENSE)。

**世界是唯一的事实源。编写它。证明它。看着它动起来。**
