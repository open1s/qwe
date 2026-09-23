# PWE 语言使用指南

[English](lang-usage.md)

PWE 语言是 `pwe-reference` 的文本前端（`src/lang.rs`，语法文件 `src/lang.pest`）。
源程序声明世界模型和系统，编译为低层 EIR，然后跨后端执行——解释器和 CPU JIT
在每一步都必须产生字节级一致的写入。

```
PWE 源码 ──parse──▶ WorldModel + 系统声明 ──lower──▶ EIR ──interpret/JIT──▶ 写入
```

```rust
let compiled = pwe_reference::lang::compile(SOURCE)?;      // 解析 → 降级 → EIR
let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
rt.step_cross()?;                                          // interpreter == JIT，每步断言
rt.step_cross_n(30)?;                                      // 一次执行 30 步
```

在 shell 里，`pwe` 命令行工具（crate `pwe-cli`）可编译、运行、演示程序：

```sh
pwe compile scene.pwe -o scene.pweb   # .pwe 源码 → 已校验的 .pweb 二进制
pwe run     scene.pweb --steps 600    # 运行二进制，每步跨后端断言
pwe present scene.pweb --port 8000    # 浏览器实时 3D 查看器
```

## 程序结构

```pwe
world {
    # 世界项（必需段）
}
funcs {
    # 可选：用户自定义纯函数
}
systems {
    # 可选：作用于世界的系统
}
```

注释：`#` 或 `//` 到行尾。空白符不影响解析。

## world 段

| 语句 | 含义 |
| --- | --- |
| `gravity = (x, y, z)` | 全局均匀重力向量。 |
| `chan <name> { value = v }` | 通道实体；最新值保存在 `state[0]`。 |
| `field <name> { width = w; height = h; dx = d }` | 确定性标量网格场（PDE 基底）：规则经 `fget`/`fset`/`flap` 读写单元。 |
| `params { G = 1.0; k = 3.0 }` | 运行时可设置的模型参数；规则按名读取，可用 `pwe run --param G=2` 覆盖（同一工件、不同配置）。 |
| `import "pkg/mod"` | Python 式模块导入：加载 `pkg/mod.pwe`（目录则加载其 `__init__.pwe` 包），并对其**函数与参数**做命名空间限定（`mod.f(...)`、`mod.G`）。`import "mod" as m` 绑定 `m`；`from "mod" import f, G` 直接绑定裸名。实体 / 系统 / 场合并进同一个世界（重名报错）。 |
| `title = "..."` | 运行标题，`pwe present` 展示。 |
| `entity <name> { fields }` | 一个物体。字段见下表。 |

### 实体字段

| 字段 | 含义 |
| --- | --- |
| `position = (x, y, z)` | 初始位置。 |
| `velocity = (x, y, z)` | 初始线速度。 |
| `state = (v0, v1, …)` | 通用状态槽（位置式）。每实体最多 16 个槽（`s0…s15`）。 |
| `state = (x = 0, y = 0, …)` | 命名状态槽；规则里按名赋值，用 `@self.x` 读取。同一列表里允许命名与位置式混用。 |
| `state = (vec3 pos, …)` | 向量元素占用 N 个连续槽，命名为 `pos`、`pos.0` … `pos.{N-1}`（零初始化）；`pos` 读分量 0，`@self.state.pos.1` 读分量 1，`s[i]` 动态索引。 |
| `mass = v` | 质量（驱动 `nbody`；查看器按它推导视觉尺寸）。 |
| `dynamic = false` | 静态物体（默认是动态的）。 |
| `nbody = false` | 把该物体排除出 `nbody` 系统。 |
| `restitution = v` | 接触弹性。 |
| `friction = v` | 接触摩擦。 |
| `box = (dx, dy, dz)` | 盒形碰撞体。 |
| `sphere = r` | 球形碰撞体。 |
| `hull = [(x,y,z), …]` | 凸包碰撞体；至少 4 个点。 |
| `camera = true` | 标记为查看器相机（不参与仿真）。 |
| `color = 0xRRGGBB` | 3D 查看器的呈现颜色。 |

实体 id 按声明顺序从 1 开始；通道 id 排在实体之后。

## 系统段

| 系统 | 参数 | 含义 |
| --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | 对动态物体施加均匀重力。 |
| `integrate` | `dt` | 把速度积分为位置。 |
| `damping` | `factor` | 每步缩放速度。 |
| `ground_contact` | `restitution` | 解算与地面平面的接触。 |
| `wall` | `x`, `z`, `y_min?`, `restitution?` | 有界域：`|x|,|z| ≤ limit`，撞墙时速度按弹性反射。 |
| `force` | `ax`, `ay`, `az`, `dt` | 对动态物体施加恒定加速度。 |
| `linear` | `slots`, `dt`, `row0 = (a0, …, c)` | 线性动力系统：`s_N' = Σ_j a_j·s_j + c`。每行 `rowN` 有 `slots + 1` 项（系数 + 常数）。 |
| `nbody` | `G`, `dt` | 动态物体间的平方反比力：`G > 0` 引力，`G < 0` 库仑斥力。 |
| `send` | `chan = name`, `value = expr` | 每个动态物体求值 `value` 并写入通道实体。 |
| `recv` | `chan = name`, `slot = n` | 把通道最新值读入每个动态物体的 `sN`。 |
| `update` | `on = name?`, `when = expr?`, `every = n?`, `substeps = n?`, `dt`, `let …`, 槽规则 | 用户自定义非线性动力系统（显式欧拉，见下）。 |
| `rk4` | `on = name?`, `when = expr?`, `every = n?`, `dt`, `let …`, 槽规则 | 与 `update` 相同的规则，但用经典 **4 阶龙格-库塔** 方法积分——在相同 `dt` 下对振荡器与非线性 ODE 精度高得多。 |
| `invariant` | `on = name?`, `expr`, `let …` | 每步断言：系统跑完后 `expr` 对被检查实体必须非零；违反不变式时该步报错（detail 69），且在任何写入应用之前失败。 |
| `watch` | `on = name?`, `expr`, `mem = slot`, `into = slot` | 零穿越检测：被监测表达式在相邻两步之间变号时置 1；上一值存于 `mem` 槽（世界状态），标志写入 `into`。 |
| `diffuse` | `field = name`, `rate` | 网格场的显式扩散：每步 `T += rate·∇²T`，使用 Jacobi 扫描（零通量模板下总严格守恒）。 |
| `poisson` | `field = name`, `source = name?`, `iters`, `scale = s?` | 对 `∇²φ = ρ·scale` 做 Gauss–Seidel 松弛——每步 `iters` 次扫描，边界单元固定。 |
| `wave` | `field = name`, `prev = name`, `velocity = c`, `dt`, `damping = s?` | 二阶蛙跳 `u_tt = c²∇²u`，跨越两个场（`prev` 保存 `u(t−h)`）；Courant 数 2D `c·h/dx ≤ 1/√2`、3D `≤ 1/√3`。`damping`（默认 `1.0`，无损）缩放时间项；`absorb` + `absorb_width` 在边界加渐变海绵层，吸收外传波而非反射。 |

未知系统种类报错（detail code 49）。

## update / rk4 系统

两种系统都接受形如 `slot = expr` 的规则。`update` 含义是 **`slot += dt · expr(state)`**
（显式欧拉）；`rk4` 每步把同一导数求 4 次再合并（Runge-Kutta 4），得到 4 阶精度——
相同 `dt` 下漂移小得多。所有读取先发生——自身状态槽、跨实体引用、属性引用每步读一次，
因此同一步内各条规则之间是同时更新的（规则顺序不影响结果）。

```pwe
systems {
    update { on = target; dt = 0.02
        let omega = 0.5
        tx = -@self.ty * omega        # tx' = -ty·ω（圆周运动）
        ty = @self.tx * omega
    }
}
```

* `on = <name>` 把规则限定在一个实体上（默认作用于所有动态物体）。
* `let name = expr` 在槽规则运行前先算好一个可复用的局部值。
* `when = expr` 门控每条规则的写入：为 0 时状态不动——状态机语义
  （`when = mode == 1`）。
* `every = n` 仅当 `step % n == 0` 时运行该系统（调度；步数经 `STEP` opcode
  读取，确定性）。
* `substeps = n`（仅 update）把积分重复 n 次、每次 `dt/n`；每个子步重读状态并
  重算 `let` 局部值，更细的欧拉更贴近解析解。跨实体引用与属性每步采样一次。
* 规则左侧（LHS）是位置式槽 `sN`，或该实体 `state = (x = 0, …)` 布局里的命名槽；
  规则按实体逐个解析。
* 引用里出现未知实体名时不产生耦合；解析不到的槽读到 `0`。

## invariant 系统

```pwe
systems {
    update { on = reactor; dt = 0.0005
        let k = 3.0 * exp(-900.0 / temp)
        na = -k * na * water
        naoh = k * na * water
    }
    invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
}
```

表达式在**系统跑完后**逐实体求值，必须非零。为零（或 NaN）时该步失败——
`step_*` 返回错误（为零时 detail 69；NaN 走 EIR 自身的比较拒绝）——**且在任何
写入应用之前失败**，场景保持本步之前的状态，绝不静默越过坏状态。

* `on = <name>` 把检查限定在一个实体上（默认作用于所有动态物体）。
* 与 `update` 一样支持 `let` 局部值。
* 每个 `invariant` 系统拥有自己的判定字段，多个可共存。

## watch 系统

```pwe
systems {
    update { on = ball; dt = 0.01
        x = vx
    }
    watch { on = ball; expr = x; mem = 5; into = 6 }
}
```

每步 watch 在**系统跑完后**求值 `expr`，与上一步的值（存于 `mem` 状态槽——
普通世界状态，持久且确定）比较，向 `into` 写入 0/1 标志：值变号（严格零穿越；
零内存是初始状态、不算穿越）时为 1。实体自身的规则读取该标志并反应——反弹、
重初始化、切换模式。NaN 经 EIR 自身的比较拒绝使该步失败。

## 表达式

| 形式 | 含义 |
| --- | --- |
| `s0`, `s1`, … | 自身状态槽。 |
| `x`（命名槽）、`@self.x` | 自身命名状态槽。 |
| `@name.sN`, `@name.state.x`, `@name.x` | 另一实体的状态槽。 |
| `@name.mass`, `@name.is_dynamic` | 另一实体的属性。 |
| `@name.position.x/y/z`, `@name.velocity.x/y/z` | 另一实体的变换/速度。 |
| `+ - * /`、一元 `-` | 算术（f64）。 |
| `< <= > >= == !=` | 比较，结果为 `1.0` / `0.0`。 |
| `and`/`&&`、`or`/`\|\|`、`not`/`!` | 逻辑连接词（非零即真），结果为 `1.0`/`0.0`。优先级：`not` > `and` > `or` > 比较。 |
| `pi`, `e` | 常数。 |
| `t` | 全局仿真时钟（秒）。 |

裸名字若既非局部值、槽，也非参数，则读到 `0.0`（未解析引用约定），不会令该步
失败。系统参数（如 `dt`）**不在表达式作用域内**——请显式写出步长（例如
`dt = 0.5` 时用 `x = (target - x) / 0.5` 把 `x` 吸附到 `target`）。

### 内建函数

* 1 元：`sin cos exp ln sqrt abs floor ceil round sign log10 log2 sinh cosh tanh asin acos atan`
* 2 元：`pow atan2 hypot min max`
* `if(c, a, b)`——选择；`random()`——有种子、可复现的 `[0,1)` 随机数；
  `noise()`——有种子标准正态（Box-Muller，两次抽取），始终有限；
  `print(x)`——记录 `x` 并原样返回，不改变世界状态；
  `emit(kind, payload)`——发出一个有序事件，返回 `0.0`；
  `last_event(kind)`——本步已发出的该 kind 最近事件的 payload（无则 0）——
  语言内事件消费。事件每步清空（宿主经 `emitted_events()` 读本步事件）；
  kind 是数值而非位模式。

### 向量助手

* `vlen(x, y, z)`——√(x²+y²+z²)；`vdot(x1,y1,z1, x2,y2,z2)`——分量点积；
  `vdist(x1,y1,z1, x2,y2,z2)`——两点距离。纯算术（无新 opcode）；与
  `neighbor_count`/`nearest_dist` 组合可表达空间模型。

### 动态槽索引

* `s[i]` 读运行时索引处的 State 槽；`s[i] = expr` 写它（`s[i] += dt·expr`，
  与所有规则一致）。索引可为任意表达式（槽、局部值、算术）。无需额外状态
  即可使用至 16 槽上限的数组。仅在 `update` 规则及其 `let` 块内有效；
  `rk4` 中的动态 LHS 暂被拒绝（仅 update 支持）。

### 空间查询

* `neighbor_count(r)`——与当前实体位置距离在 `r` 以内的其他实体个数。
  参与者：所有非相机场景物体；位置取 `Transform` 或 `state[0..2]`（与查看器
  约定一致）。确定性（按 id 排序扫描）。
* `nearest_dist()`——到最近其他实体的距离；当前实体独存时为 `f64::MAX`。
  确定性。
* `neighbor_mean(slot, r)`——半径 `r` 内邻居的 State 槽 `slot` 的均值（无则 0）。
  对槽 0/1/2 取位置均值、3/4/5 取速度均值——聚合/对齐（flocking）原语。
* `nearest_dx/dy/dz()`——各轴上 `(最近邻居 − 自身)` 的偏移（独存为 0），
  规则可据此靠近或远离最近体。
* 仅在系统规则及其 `let` 块内有效——函数体内无效（无实体上下文；detail 70）。

### 模块与包

每个 `.pwe` 文件是一个**模块**。`import` 遵循 Python 语义：

```pwe
import "physics"                 # physics.G、physics.thrust(m)
import "physics" as ph           # ph.G
from "physics" import thrust     # thrust(m)（裸名）
```

* **包**即目录：`import "shapes"` 加载 `shapes/__init__.pwe`；嵌套路径
  `import "lib/kepler"` 加载 `lib/kepler.pwe`。
* **函数与参数**按模块加命名空间：模块自身的规则先在其命名空间内解析裸名、
  再回退到全局。实体、系统、场、通道属世界内容、扁平合并（跨模块实体/场重名
  即编译错误）。
* **循环导入可解析**：模块只加载一次并合并，且其成员按**每个别名**注册，因此
  互相引用（`a` ↔ `b`）与多别名引用（`import "x" as alpha` 与 `import "x"` 并存）
  都能解析。`--param` 会同时更新同一参数的所有别名。缺文件、实体/场重名会报错
  （detail 76）。

### 计划事件（离散事件调度）

计划事件**精确触发一次**：当某步的时间窗 `[t, t + dt)` 覆盖预定时刻时触发——
确定性、无状态、与积分方法无关。

* `at(T)`——到达时间 `T` 的那一步为 1.0，否则 0.0。
* `periodic(P)` / `periodic(P, phase)`——每 `P` 秒触发一次（要求 `P > dt`）。
* 与规则组合成脉冲、与 `emit`/`last_event` 组合成事件驱动反应。

### 单位（渐进式量纲分析）

单位为**可选、编译期检查**。未标注的值是通配符、永不报错，因此无单位模型不受影响。
标注写在值后的方括号内：

```pwe
world {
    params { k = 4.0 [1/s^2] }
    entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
}
systems {
    update { on = e; dt = 0.1 [s]
        vx = 0.0 - k * x     # 1/s^2 · m · s = m/s  与 vx 一致
        x = vx               # m/s · s = m          与 x 一致
    }
}
```

基本单位：`m`、`kg`、`s`、`A`、`K`、`mol`、`cd`；用 `*`、`/`、`^` 组合。
每条规则按 `slot += dt · expr` 检查（`dt` 单位已声明则用之，否则默认秒）；
不一致即编译错误（detail 77）。超越函数要求无量纲参数；`sqrt` 指数减半。

### 网格场（PDE 基底）

在 world 段用 `field <name> { width = w; height = h; dx = d }`（2D）或
`field <name> { width = w; height = h; depth = d; dx = h }`（3D）声明；单元是
确定性世界状态（像其他世界状态一样可快照/重放）。空间是 3D——加上仿真时钟，
场即 4D 基底（3D 空间 + 时间）。

* `fget(f, i, j)` / `fget(f, i, j, k)`——单元值（2D / 3D）；能看到同一步内的写入。
* `fset(f, i, j, v)` / `fset(f, i, j, k, v)`——写单元（裸调用语句；返回 `0.0`）。
* `flap(f, i, j)` / `flap(f, i, j, k)`——Field 的零通量模板离散拉普拉斯，按
  `1/dx²` 缩放（2D 五点、3D 七点）——热/扩散/Poisson 规则组合的 PDE 算子。
* 坐标可为任意表达式（槽、局部值、算术）。未知场名读到 `0`（已文档化的
  未解析引用约定）。

#### 连续场求解器（`diffuse` / `poisson`）

无需手写 `fget`/`fset`/`flap` 循环，直接声明求解器：

```pwe
field heat { width = 32; height = 32; dx = 1.0 }
field phi  { width = 32; height = 32; dx = 1.0 }
field rho  { width = 32; height = 32; dx = 1.0 }
field u    { width = 64; height = 64; dx = 1.0 }
field um   { width = 64; height = 64; dx = 1.0 }
systems {
  diffuse { field = heat; rate = 0.2 }                       # T += 0.2·∇²T
  poisson { field = phi; source = rho; iters = 20 }          # ∇²φ = ρ
  wave    { field = u; prev = um; velocity = 1.0; dt = 0.5 } # u_tt = c²∇²u
}
```

`diffuse` 先对全部单元及其拉普拉斯取自同一快照、再统一写回——即 Jacobi
扫描，故注入总量严格守恒（2D 稳定条件 `rate ≤ 1/4`，3D `≤ 1/6`）。`poisson` 每步做
`iters` 次就地 Gauss–Seidel 扫描；边界单元相当于固定电势（用 `fset` 设置）。
`wave` 每步平移两个场（`prev ← u`、`u ← 2u − prev + (c·h/dx)²∇²u`），因此初始
脉冲会分裂为球面（3D）或圆环（2D）波前。`depth > 1` 时所有求解器按 3D 迭代。
每步**只跑一次**（仅首个动态实体发射
扫描），确定性，且降级为既有场指令，解释器与 JIT 保持逐字节一致。

### 标准库（`std/`）

`std/` 是一组用于通用仿真的纯函数模块：

```pwe
import "std/forces"
import "std/thermal"
systems {
  update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
  }
}
```

模块：`math`、`particles`、`forces`、`mechanics`、`chemistry`（元素周期表
1–118）、`thermal`、`acoustics`、`optics`、`em`、`robotics`、`units`、`control`。
物理常量以参数形式给出
（`chemistry.R_gas`、`thermal.sigma_sb`、`em.k_coulomb` 等），可用
`--param` 覆盖。完整 API 见 `std/README.md`，组合示例见
`cli/examples/domains.pwe`。

### 用户自定义函数

```pwe
funcs {
    clamp(a, lo, hi) { if(s0 < s1, s1, if(s0 > s2, s2, s0)) }
}
```

函数体是一个标量表达式；参数按槽引用——`s0`、`s1`、…（第 *i* 个参数是槽
`s_i`）。函数降级为 EIR `CALL`，可从任何 `update` 规则和 `send` 的 value 调用。
元数在编译期校验。

### 分支控制

条件用逻辑连接词组合，用 `if` 分支：

```pwe
systems {
    update { on = heater; dt = 0.1
        # 带滞回的 bang-bang 恒温器（18–20 °C）
        let on = if(temp < 18.0, 1.0, if(temp > 20.0, 0.0, h))
        temp = on * 1.2 - (temp - 16.0) * 0.06
        h = on - h
    }
}
```

`not` 绑定到其后的因子：取反比较请写 `not (x > 0)`。

### 循环

`repeat n { … }`、`for i in lo..hi { … }` 和 `break`/`continue` 语句在降级期
展开（有上界），因此仍符合 EIR 的直线式（SSA、无回边）契约：

```pwe
systems {
    update { on = solver; dt = 1.0
        let g = s1
        repeat 100 until (abs(g * g - s0) < 1e-12) {
            let g = (g + s0 / g) * 0.5       # 对 sqrt(s0) 的牛顿迭代
        }
        s1 = g - s1                          # 把收敛值写回
    }
}
```

* 循环体内只能有 `let`、嵌套循环和 `break`/`continue`；槽规则留在循环外
  （语法强制）。
* `repeat n until (cond)` 在每次迭代**之后**检查条件（为真则退出）；
  `repeat n while (cond)` 在**之前**检查（为假则退出）。`break`/`continue`
  可带可选的 `if (cond)`。
* `break` 只退出最内层循环。被守护的表达式仍会求值，其结果由门丢弃——
  IEEE f64 求值无陷阱，因此在直线式 EIR 中安全。
* 上限：单循环最多迭代 1000 次、展开最多 10000 条语句。
* `let` 名不得遮蔽保留 token（`t`、`pi`、`e`、`s0`、`s1`、…）——语法先解析
  这些名字，这样的绑定永远读不回来。

## 语义注意

* 时间是显式的：`dt` 乘以每条规则的表达式；仿真时钟 `t` 每步前进 `dt`。
* 确定性是一等公民：`random()` 有种子、可复放；解释器与 JIT 每步字节级一致
  （`step_cross`）。
* 每个实体的状态槽上限 16 个（`MAX_STATE_SLOTS`）。

## 通用仿真基底

因为规则是作用在命名/位置式状态槽上的普通标量 ODE，再加上 `let` 局部变量、
用户函数、跨实体引用、`random()`/`emit()`，以及 RK4 或欧拉积分，语言并不局限于
刚体物理。任何可表达为（耦合）微分或差分方程的规律——物理、化学或生物——都能
建模、仿真与可视化。参见 `reference/examples/scientific_laws_demo.rs` 的实时画廊：
一次运行五条定律（简谐运动、开普勒轨道、可逆动力学、logistic 增长、放射性衰变），
每条都由 `reference/tests/laws.rs` 验证。
* 先读后写：同一步内，每个被引用的值都是这一步**开始时**的值。

## 错误 detail code

| Code | 含义 |
| --- | --- |
| 48 | 缺少必需的系统参数。 |
| 49 | 未知系统种类。 |
| 51 | 凸包至少需要 4 个点。 |
| 52 | 状态槽数量超出范围（1..=16）。 |
| 53 | 缺少 `linear` 的行。 |
| 54 | `linear` 行长度 ≠ `slots + 1`。 |
| 55 | `funcs` 函数体非法 / `update` 规则为空 / 槽左侧非法。 |
| 56 | 表达式解析失败。 |
| 57 | 数字解析失败。 |
| 58 | 槽 / 引用解析失败。 |
| 59 | 调用元数不符。 |
| 60 | 程序解析失败。 |
| 62 | 未知的实体或通道名。 |
| 64 | 颜色字面量非法。 |

## 诊断

编译失败会携带人类可读信息、detail code 与源码偏移。参考实现通过 `lang` 模块暴露：

```rust
match pwe_reference::lang::LangRuntime::compile(source) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(source, &e)),
}
```

`lang::diagnose` 渲染多行报告，带出错源码行与脱字符，例如缺失参数：

```text
error 48: system 'update' is missing required parameter 'dt'
  --> line 13, column 9
    |
  13 |         update { on = reactor; dt = 0.0005
    |         ^
```

* `lang::clear_diagnostics` / `lang::take_diagnostics` 取回一次失败编译的原始
  `Diagnostic` 列表（`detail`、`message`、`byte_offset`）。
* `lang::detail_name(detail)` 把 code 映射为规范短语。
* 注释（`#` / `//` 到行尾）作为空白在程序任意位置被跳过，包括 `update` /
  `rk4` 规则块内。

## 配方

最简单的下落体（`language_demo`）：

```pwe
world {
    gravity = (0, -9.81, 0)
    entity vehicle { position = (0, 8, 0); velocity = (4, 0, 0); mass = 4; dynamic = true; box = (1, 0.5, 0.7) }
    entity ground  { position = (0, -5, 0); dynamic = false; box = (50, 5, 50) }
}
systems {
    gravity { gravity_y = -9.81; dt = 1 / 60 }
    integrate { dt = 1 / 60 }
    ground_contact { restitution = 0.6 }
}
```

放射性衰变（linear）：

```pwe
world { gravity = (0,0,0) entity isotope { state = (100, 0) } }
systems { linear { slots = 2; dt = 1; row0 = (-0.05, 0, 0); row1 = (0, 0, 0) } }
```

非线性摆（update + `sin`）：

```pwe
world { gravity = (0,0,0) entity pend { state = (1.2, 0) } }
systems { update { dt = 0.0005
    s0 = s1
    s1 = -9.81 * sin(s0) } }
```

轨道系统（nbody，`solar_demo`）：

```pwe
world {
    gravity = (0, 0, 0)
    entity sun  { state = (0, 0, 0, 0, 0, 0, 1000000, 0); color = 0xFFD24A }
    entity earth{ state = (4, 0, 0, 0, 500, 0, 1, 0);     color = 0x4aa8ff }
}
systems { nbody { G = 1.0; dt = 0.0001 } }
```

`state[0]` 是轨道半径向量的 x 分量，`state[3]` 是初始轨道速度；槽 6 是质量
（查看器按它推导视觉尺寸）。
