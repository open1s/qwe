# PWE 语言使用指南

[English](lang-usage.md)

本指南**自包含**：仅凭本文件即可编写、运行与调试完整的 PWE 程序。对应 PWE
0.0.x（`pwe-reference` + `pwe-cli`）。

> **若只读两节，请读 §0.3（两种执行模型）与 §6（容易踩的坑）。** 几乎所有“反直觉”
> 的 bug 都源于这两节。

---

## 0. 总览

### 0.1 PWE 是什么

PWE 是一门小巧、确定性的仿真语言。程序声明**世界模型**（实体、场、参数）与
**系统**（行为），编译为低层 IR（EIR），然后**跨后端**执行——解释器与 CPU JIT
每一步都必须产生**字节级一致**的写入。

```
源码 ──parse──▶ 世界模型 + 系统 ──lower──▶ EIR ──interpret ≡ JIT──▶ 提交写入
```

可依赖的结论：
* **确定性是一等公民**：同源码 + 同种子 ⇒ 每次、每个后端都得到相同轨迹；
  `random()`/`noise()` 有种子。
* 世界是唯一真相源；一步是 **Observe → Compute → (Prepare) → Commit → Publish**。
  写入在步末原子提交；不变式在**任何写入之前**判定并可使该步失败。

### 0.2 快速上手

```sh
cargo build --release -p pwe-cli

./target/release/pwe compile scene.pwe -o scene.pweb   # 源码 → 已校验工件
./target/release/pwe run     scene.pweb --steps 600    # 运行 N 步
./target/release/pwe present scene.pweb --port 8000    # 浏览器实时 3D 查看器
# 可选：--param K=V 在 run/present 时覆盖已声明的模型参数
```

```rust
// 嵌入 API
let mut rt = pwe_reference::lang::LangRuntime::compile(src)?;  // 或 ::compile_file(path)
rt.step_cross()?;        // 一步，断言 interpreter == JIT
rt.step_cross_n(600)?;   // 多步
let frame = rt.present_frame(None);   // 渲染快照
```

最小程序：

```pwe
world {
  gravity = (0, -9.81, 0)
  entity ball { position = (0, 5, 0) velocity = (3, 0, 0) sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (20, 1, 20); color = 0x557755 }
}
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.6 }
}
```

### 0.3 两种执行模型（务必阅读）

物体的动力学数据存在**两套互相独立**的存储中，一个物体选择其中一种。混用是“物体
不动”类 bug 的常见根源。

| | **分量型物体** | **状态槽型物体** |
| --- | --- | --- |
| 声明方式 | `position = (…)`、`velocity = (…)`、`mass`、`dynamic`、碰撞体（`box`/`sphere`/`hull`） | `state = (…)` |
| 由哪些系统驱动 | `gravity`、`integrate`、`damping`、`force`、`wall`、`ground_contact` | `update`、`rk4`、`linear`、`nbody` |
| 位置存于 | `Transform.position` | `state[0..2]`（渲染回退） |
| 速度存于 | `Velocity.linear` | `state[3..6]`（自行约定） |
| 规则中读写 | `@name.position.x`、`@name.velocity.x` | `x`、`@name.x`、`s0`… |

* **分量型**由内建物理系统驱动，通常**不需要**为它写 `update`。
* **状态槽型**由 `update`/`rk4` 规则积分；`position` 可选，若同时驱动 `state[0..2]`
  则仅为渲染回退。
* `nbody` 是状态槽系统：约定 `state = (px, py, pz, vx, vy, vz, m)`（质量在**槽 6**，
  不是 `mass` 字段）。见 `cli/examples/solar.pwe`。
* `joint`/`soft` 读取 **`Transform` 位置与 `rigid_body.mass`**，因此其物体应为分量型
  （`position`、`mass`、`dynamic`）。

### 0.4 确定性、渲染与状态哈希

* 世界状态 = 实体（transform/velocity/state/`active`）+ 场 + 参数 + 时钟。快照与
  `state_hash` 正好覆盖这些。
* 呈现属性（`color`、`shape`、`size`、`opacity`、`glow`、`label`、`orient`、
  `vector`、自定义形状）**不进入**状态哈希。

---

## 1. 词法、关键字与语法

### 1.1 词法规则

| 词法单元 | 形式 | 说明 |
| --- | --- | --- |
| 注释 | `# …` 或 `// …` | 到行尾；任意位置跳过 |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | 实体/槽/参数/函数/形状名 |
| `number` | `-? 数字 ("." 数字)? (("e"\|"E") "-"? 数字)?` | f64 字面量 |
| `value` | `number`（`/ number`）? | 字面量**或比值**（`dt = 1/60`） |
| `boolean` | `true` \| `false` | |
| `string` | `"…"`（无转义） | 标题、SVG 路径 |
| `color` | `0x` 十六进制 | `0xRRGGBB` |
| `unit` | `[ m/s^2 ]` | 基本单位 `m kg s A K mol cd`，算符 `* / ^`；仅编译期 |
| `slot` | `s` 数字 | 按位置引用自身状态槽（`s0`、`s1`…） |
| 常量 | `t`、`pi`、`e` | 时钟（秒）、π、自然常数 |

空白不影响解析；语句/参数之间的 `;` **可选**。单位必须带方括号，以免与 `s[0]`
混淆。

### 1.2 关键字（保留——不能作标识符）

* 段：`world`、`funcs`、`systems`
* world：`gravity`、`title`、`params`、`chan`、`value`、`entity`、`field`、
  `pool`、`soft`、`width`、`height`、`depth`、`dx`、`nx`、`ny`、`nz`、`spacing`、
  `origin`、`shape`、`part`
* 实体字段：`position`、`velocity`、`state`、`vec`、`mass`、`dynamic`、`nbody`、
  `parent`、`restitution`、`friction`、`box`、`sphere`、`hull`、`rotation`、
  `camera`、`color`、`size`、`opacity`、`glow`、`label`、`orient`、`vector`
* 形状：`point`、`sphere`、`box`、`capsule`、`svg`、`hull`、`poly`、`at`、`depth`、
  `scale`、`faces`
* 控制：`return`、`let`、`repeat`、`until`、`while`、`for`、`in`、`break`、
  `continue`、`if`
* 逻辑：`and`、`or`、`not`、`&&`、`||`、`!`、`true`、`false`
* 原子：`pi`、`e`、`t`

**上下文名（非保留）**：系统*种类*及其*参数*（`update`、`on`、`when`、`every`、
`substeps`、`dt`、`field`、`pool`、`body`、`type`…）都是普通标识符、在构建期匹配。
内建函数名（`sin`、`min`、`random`…）是普通调用、在降级期特判。`import`/`as`/
`from` 由模块加载器处理。

### 1.3 关于 `slot = …` 的一条经验

在系统内，`name = <数字>`（**裸数字**）会被解析为**系统参数**而非规则。要写常量
表达式，请写成非平凡形式：`name = 0.0 + 3.0`。详见 §6。

### 1.4 运算符优先级

由高到低：一元 `-`、`not`/`!` → `* / %` → `+ -` → 比较 `< <= > >= == !=` →
`and`/`&&` → `or`/`||`。比较与逻辑结果为 `1.0`/`0.0`；非零为真。`not` 绑定其后的
因子——取反比较写 `not (x > 0)`。

---

## 2. 程序结构与模块

```pwe
world   { # 必需：实体、场、参数、形状、池、软体 }
funcs   { # 可选：纯标量函数 }
systems { # 可选：行为 }
```

每个 `.pwe` 文件是一个**模块**。导入遵循 Python：

```pwe
import "physics"               # physics.G、physics.thrust(m)
import "physics" as ph         # ph.G
from "physics" import thrust   # thrust(m)（裸名）
```

**导入路径相对当前文件解析**（例如在 `cli/examples/` 中写 `import "../../std/forces"`；
仅当 `std/` 就在你的文件旁时 `import "std/forces"` 才成立）。**包**即目录
（`import "shapes"` → `shapes/__init__.pwe`）。函数与参数按模块加命名
空间；模块自身规则先在其命名空间内解析裸名、再回退全局。实体/系统/场/通道扁平
合并（重名报错，detail 76）。标准库位于 `std/`（如 `import "std/forces"`）。

---

## 3. `world` — 世界模型

| 语句 | 含义 |
| --- | --- |
| `gravity = (x, y, z)` | 全局均匀重力（分量型物体） |
| `title = "…"` | 查看器显示的运行标题 |
| `params { G = 1.0; k = 3.0 }` | 模型参数；规则按名读取，可用 `--param G=2` 覆盖 |
| `chan <name> { value = v }` | 通道实体（值存于 `state[0]`） |
| `entity <name> { … }` | 一个物体（§3.1） |
| `shape <name> { part … }` | 自定义渲染形状（§3.2） |
| `field <name> { width=w; height=h; dx=d; depth? }` | 确定性标量网格（§3.3） |
| `pool <name>[N] { … }` | N 个初始未激活的槽，用于动态实体（§3.4） |
| `soft <name> { … }` | 质量-弹簧软体（§3.5） |
| `import "…"` | 模块导入（§2） |

**实体 id 顺序**（到处适用，含池/软体命名）：已声明实体 `1..E`，然后通道，然后池槽
（`<pool>#k`），然后软体粒子（`<soft>#k`）。

### 3.1 实体字段

| 字段 | 含义 |
| --- | --- |
| `position = (x, y, z)` | 初始位置（Transform）。 |
| `velocity = (x, y, z)` | 初始线速度。 |
| `state = (v0, v1, …)` | 位置式状态槽（最多 16：`s0…s15`）。 |
| `state = (x = 0, y = 0, …)` | 命名状态槽：按名赋值、`@self.x` 读取；可与位置式混用。 |
| `state = (vec3 pos, …)` | 向量元素占 N 个连续槽，命名 `pos`、`pos.0`…`pos.{N-1}`。 |
| `mass = v` | 质量（驱动 `nbody` 与视觉尺寸；`joint`/`soft` 必需）。 |
| `dynamic = false` | 静态物体（默认动态）。 |
| `nbody = false` | 排除出 `nbody` 系统。 |
| `parent = <name>` | 卫星（相对显示）。 |
| `restitution = v` / `friction = v` | 接触弹性 / 切向摩擦。 |
| `box = (dx,dy,dz)` / `sphere = r` / `hull = [(x,y,z), …]` | 碰撞体（凸包 ≥ 4 点）。 |
| `rotation = (rx, ry, rz)` | **静态**欧拉旋转（弧度，XYZ），如倾斜地面。 |
| `camera = true` | 查看器相机（不参与仿真）。 |

**仅呈现字段**（不影响仿真状态、确定性或哈希）：

| 字段 | 含义 |
| --- | --- |
| `color = 0xRRGGBB` | 渲染颜色。 |
| `shape = point\|sphere\|box\|capsule\|<自定义>` | 渲染形状（覆盖碰撞体推导）。 |
| `size = v \| (dx,dy,dz)` | 标记直径 / 半径 / 盒边长 / 按轴盒尺寸；对自定义形状整体缩放。 |
| `opacity = v` | `[0, 1]`。 |
| `glow = v` | 自发光强度（0=哑光）。 |
| `label = false` | 隐藏浮动名称（默认显示）。 |
| `orient = true` | 对纯 state 实体，将 state 槽 **7/8/9** 作为欧拉 **(pitch, yaw, roll)** 弧度用于渲染朝向（yaw 绕 Y，pitch 在身体局部）。默认槽 7 为绕 Z 自转。 |
| `vector = false` | 隐藏查看器速度箭头 / 轨道环（当槽 3–5 不是速度时）。 |

### 3.2 自定义形状

形状是一组部件，实体用 `shape = <名>` 引用。

```pwe
world {
  shape head { part sphere = 0.115 at (0, 0, 0); }
  shape drone {
    part capsule = (0.06, 0.30, 0.06);
    part sphere  = 0.09 at (0, 0.20, 0);
    part box     = (0.54, 0.02, 0.02) at (0, 0.20, 0);
  }
  shape octa { part hull = [(0,0.9,0),(0.9,0,0),(0,-0.9,0),(-0.9,0,0),(0,0,0.9),(0,0,-0.9)]; }
  shape gem {
    part poly = [(0,0.9,0),(0.7,0,0.7),(-0.7,0,0.7),(-0.7,0,-0.7),(0.7,0,-0.7),(0,-0.9,0)]
      faces = [[0,1,2],[0,2,3],[0,3,4],[0,4,1],[5,1,4],[5,4,3],[5,3,2],[5,2,1]];
  }
  shape star {
    part svg = "M 0,-1 L 0.224,-0.309 L 0.951,-0.309 L 0.363,0.118 L 0.588,0.809 L 0,0.382 L -0.588,0.809 L -0.363,0.118 L -0.951,-0.309 Z" depth 0.22 scale 0.8;
  }
  shape person {                       # 组合：包含另一个形状
    part box  = (0.34, 0.46, 0.20) at (0, 0.57, 0);
    part head at (0, 0.92, 0);
  }
}
```

* `sphere = r`、`box = (dx,dy,dz)`、`capsule = (底半径, 长度, 顶半径)`。
* `hull = [(x,y,z), …]`——凸多面体。
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]`——任意多面体（面做三角化；可用于地形
  网格，见 §7.12）。
* `svg = "<d>" depth <d>`——SVG 路径沿 Z 挤出。
* `part <另形状> [at (x,y,z)] [scale s]`——**包含另一个形状**，递归内联（未知名称 /
  引用环为编译错误）。

`at` 为局部偏移；`scale` 为该部件的幅度；实体的 `size` 缩放整个形状。形状**仅用于
呈现**。

### 3.3 网格场——连续场基底

```pwe
field heat { width = 16; height = 16; dx = 1.0 }          # 2D
field u { width = 17; height = 17; depth = 17; dx = 1.0 } # 3D
```

单元是确定性世界状态（可快照/重放）。规则中访问：

| 调用 | 作用 |
| --- | --- |
| `fget(f, i, j)` / `fget(f, i, j, k)` | 读单元（能看到同一步内写入） |
| `fset(f, i, j, v)` / `fset(f, i, j, k, v)` | 写单元（也可作裸语句） |
| `flap(f, i, j)` / `flap(f, i, j, k)` | 零通量离散拉普拉斯，按 `1/dx²` 缩放（2D 五点、3D 七点） |

第一个参数必须是**字面量场名**。场的读取能看到同一步内更早的写入（与实体读取不同，
见 §6.4）。

### 3.4 实体池——动态实体（RFC-0038）

`pool name[N] { …实体字段… }` 是一块**预分配**槽，初始全部**未激活**；EIR 保持静态
（每实体一个函数）：

```pwe
world {
  entity emitter { state = (x = -6.0, vx = 1.5) }
  pool p[24] { state = (x = 0.0, vx = 0.0); shape = sphere; size = 0.18 }
}
systems {
  spawn   { on = emitter; pool = p }              # 每步一个空闲槽
  spawn   { on = emitter; pool = p; count = 3 }   # 批量：每步最多 3 个
  spawn   { on = emitter; pool = p; every = 2; phase = 1 }  # 带相位
  update  { on = p; dt = 0.1 x = 0.0 + vx }       # 仅在已激活槽上运行
  despawn { on = p; when = x > 6.0 }              # 回收
}
```

* 槽命名为 `<pool>#0`、`#1`…；id 排在实体与通道之后。
* 未激活槽被用户系统跳过（`active()` 守卫）并在查看器隐藏；`despawn` 仍会在其上
  运行以清除标志。
* `spawn` 激活 id 最小的空闲槽并复制**调用者**的 state；`count` = 每次发射槽数，
  `every`/`phase` 限定在 `step % every == phase` 的步发射。`active()` 读当前实体
  激活标志（0/1）。
* 任意系统中 `on = <池>` 会展开为该池全部槽（`update`、`despawn`、`invariant`…）。
* `active` 参与哈希/快照。

### 3.5 软体（RFC-0040）

```pwe
world {
  soft cloth { nx = 8; ny = 8; nz = 1; spacing = 0.4; origin = (-1, 5, 0); mass = 0.1 }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  soft      { body = cloth; stiffness = 1.0; damping = 0.3; iterations = 6 }
}
```

生成 `nx × ny × nz` 动态粒子网格（`nz` 默认 1），粒子间以结构/剪切/弯曲距离弹簧
相连；`soft` 按质量加权位置松弛求解。网格边渲染为 bond。示例：`cloth.pwe`、
`jelly.pwe`（3D）。

---

## 4. 系统

### 4.1 全部系统种类（精确参数）

`?` = 可选。缺少必需参数 ⇒ detail 48；未知种类 ⇒ detail 49。

| 种类 | 参数 | 模型 | 含义 |
| --- | --- | --- | --- |
| `gravity` | `gravity_y`、`dt` | 分量 | `velocity.y += gravity_y·dt` |
| `integrate` | `dt` | 分量 | `position += velocity·dt` |
| `damping` | `factor` | 分量 | `velocity *= factor` |
| `force` | `ax`、`ay`、`az`、`dt` | 分量 | `velocity += (ax,ay,az)·dt` |
| `wall` | `x`、`z`、`y_min?`、`restitution?` | 分量 | 在 `±x`、`±z` 边界反射 |
| `ground_contact` | `restitution` | 分量 | 解算与 `y=0` 平面接触 |
| `linear` | `slots`、`dt`、`row0 = (a0,…,c)`… | 状态 | `s_N' = Σ a_j s_j + c`；每行 `slots+1` 项 |
| `nbody` | `G`、`dt` | 状态 | 互平方反比力；需 `state = (px,py,pz,vx,vy,vz,m)` |
| `send` / `recv` | `chan`、`value` / `chan`、`slot` | 通道 | Go 式通道收发 |
| `update` | `dt`、`on?`、`when?`、`every?`、`substeps?`、规则 | 状态 | 显式欧拉 ODE 规则 |
| `rk4` | `dt`、`on?`、`when?`、`every?`、`substeps?`、规则 | 状态 | 同一规则的 4 阶 RK |
| `invariant` | `expr`、`on?` | — | 每步断言（§4.6） |
| `watch` | `expr`、`mem`、`into`、`on?` | 状态 | 零穿越标志（§4.7） |
| `diffuse` | `field`、`rate` | 场 | `T += rate·∇²T`（Jacobi，守恒） |
| `poisson` | `field`、`iters`、`source?`、`scale?` | 场 | Gauss–Seidel `∇²φ = ρ·scale` |
| `wave` | `field`、`prev`、`velocity`、`dt`、`damping?`、`absorb?`、`absorb_width?` | 场 | 蛙跳 `u_tt = c²∇²u` |
| `spawn` | `on`、`pool`、`count?`、`every?`、`phase?` | 池 | 激活空闲槽（§3.4） |
| `despawn` | `on`、`when` | 池 | 停用匹配槽 |
| `joint` | `on`、`other`、`type`、`length?`、`stiffness?`、`damping?`、`axis?`、`anchor?`、`limit?`、`iterations?` | 变换 | 成对约束（§4.8） |
| `soft` | `body`、`stiffness?`、`damping?`、`iterations?` | 变换 | 质量-弹簧网格（§3.5） |

### 4.2 `update` / `rk4`——核心

两者接受规则 `slot = expr`，含义是 **`slot += dt · expr`**（积分）。

```pwe
systems {
  update { on = target; dt = 0.02
    let omega = 0.5
    tx = -@self.ty * omega        # tx' = -ty·ω（xy 平面内旋转）
    ty =  @self.tx * omega
  }
}
```

* `on = <名>` 限定单一实体（或池，§3.4）；默认 = 所有动态物体。注意：无 `on` 时规则
  会作用于**每个**动态物体（含无关者）——建议总是写 `on`。
* `let name = expr`：规则前先算的可复用局部值。
* `when = expr`：门控所有写入（为 0 时状态不动）——状态机语义。
* `every = n`：仅当 `step % n == 0` 运行。`substeps = n`：以 `dt/n` 重复积分 n 次
  （每个子步重新读取自身状态）。
* 左侧为 `sN` **或**实体 `state = (…)` 布局中的命名槽。
* **赋值而非积分**：用 `slot = (target - slot)`（`dt=1` 时精确写入），或
  `slot = (target - slot) / dt`。

### 4.3 分量物理

`gravity` → `integrate` 是常见组合（顺序：先力/重力）。`force` 加恒定加速度；
`wall`/`ground_contact` 处理边界。它们操作 `position`/`velocity` 分量，**不是** `state`
槽。

### 4.4 通道（无线程并发）

`chan <name> { value = v }` 声明通道实体。`send { chan = c; value = e }` 发布；
`recv { chan = c; slot = k }` 把最新值读入 `state[k]`。跨运行时可经嵌入 API 路由
（`set_peer`、region）。

### 4.5 连续场求解器

```pwe
diffuse { field = heat; rate = 0.2 }                       # T += 0.2·∇²T
poisson { field = phi; source = rho; iters = 20 }          # ∇²φ = ρ
wave    { field = u; prev = um; velocity = 1.0; dt = 0.5 } # u_tt = c²∇²u
```

* `diffuse`——每步一次 Jacobi 扫描；零通量 ⇒ 总量严格守恒。稳定 `rate ≤ 1/4`（2D）/
  `≤ 1/6`（3D）。
* `poisson`——`iters` 次就地 Gauss–Seidel；边界单元固定。
* `wave`——跨两场蛙跳；Courant `c·h/dx ≤ 1/√2`（2D）/ `≤ 1/√3`（3D）。`damping`
  （默认 1.0）缩放时间项；`absorb` + `absorb_width` 加渐变海绵层而非反射。
* `depth > 1` 时按 3D；每步一次；确定性。

### 4.6 `invariant`——断言

在**系统跑完后**逐实体求值，必须非零（0/NaN 使该步失败，detail 69，且**在任何写入
之前**）。

```pwe
invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
```

### 4.7 `watch`——零穿越检测

把 `expr` 与上一步的值（存于 `mem` 状态槽，持久世界状态）比较，严格变号时向 `into`
写 1，否则 0。

```pwe
watch { on = ball; expr = x; mem = 5; into = 6 }
```

### 4.8 关节（RFC-0039）

```pwe
joint { on = a; other = b; type = distance;  length = 1.0; stiffness = 1.0 }
joint { on = a; other = b; type = spring;    length = 1.0; stiffness = 20; damping = 0.5 }
joint { on = a; other = b; type = hinge;     anchor = (0,0,0) }
joint { on = a; other = b; type = prismatic; axis = (1,0,0); limit = (0.0, 2.0) }
```

对 `Transform` 位置做迭代、质量加权的位置松弛（配合 `gravity`+`integrate`）。
`dynamic = false` 视为无限质量锚点。种类：`distance`/`spring`（保持 `|a−b| = length`）、
`weld`/`hinge`/`revolute`/`ball`/`spherical`（令 `a + anchor` 与 `b` 重合）、
`prismatic`/`slider`（令 `b` 落在过 `a`、方向 `axis` 的直线上，可选 `limit = (lo,hi)`）。
需要转动或比例的关节（`cone`、`universal`、`gear`、`rack`、`pulley`）会被拒绝（本
引擎为质点模型）。示例：`chain.pwe`。

### 4.9 表达式

| 形式 | 含义 |
| --- | --- |
| `s0`、`s1`… | 自身状态槽 |
| `x`、`@self.x` | 自身命名状态槽 |
| `@name.sN`、`@name.state.x`、`@name.x` | 另一实体状态槽 |
| `@name.mass`、`@name.is_dynamic` | 另一实体属性 |
| `@name.position.x/y/z`、`@name.velocity.x/y/z` | 另一实体变换/速度 |
| `+ - * / %`、一元 `-` | 算术（f64） |
| `< <= > >= == !=` | 比较 → `1.0`/`0.0` |
| `and`/`&&`、`or`/`\|\|`、`not`/`!` | 逻辑（非零为真） |
| `pi`、`e`、`t` | 常量 / 时钟 |

裸名字若既非局部值、槽，也非参数则读到 `0.0`。系统参数（如 `dt`）**不在**表达式
作用域内——但可 `let dt = 0.02` 后使用 `dt`。

### 4.10 内建函数（完整；元数编译期校验，detail 59）

**没有 `tan`**——请写 `sin(x)/cos(x)`。

* **1 元数学**：`sin cos exp ln sqrt abs floor ceil round sign log10 log2 sinh cosh
  tanh asin acos atan`
* **2 元数学**：`pow atan2 hypot min max`
* **选择**：`if(c,a,b)`（两支都会求值，未用一支丢弃）
* **随机（有种子）**：`random()` 于 `[0,1)`、`noise()` 标准正态
* **I/O 与事件**：`print(x)`；`emit(kind,payload)`；`last_event(kind)`（本步最近同类
  事件 payload，无则 0）
* **调度**：`at(T)`（时间窗 `[t,t+dt)` 覆盖 `T` 的那步为 1.0）；`periodic(P[,phase])`
  （每 P 秒一次；需 `P > dt`）；`schedule(gate,delay,kind,payload)`（`gate≠0` 时入队）
* **池**：`active()`（当前实体激活标志）
* **向量助手**：`vlen(x,y,z)`、`vdot(x1,y1,z1,x2,y2,z2)`、`vdist(x1,y1,z1,x2,y2,z2)`
* **空间查询**（仅规则内；否则 detail 70）：`neighbor_count(r)`、`nearest_dist()`、
  `neighbor_mean(slot,r)`、`nearest_dx/dy/dz()`（位置取 Transform，否则 `state[0..2]`）
* **网格场**：`fget`、`fset`、`flap`（§3.3）
* 其它名字 = `funcs` 中的用户函数

### 4.11 `funcs`——纯函数

```pwe
funcs {
  hooke(k, x) { 0.0 - k * x }        # 表达式体
  clamp01(v) { if (v < 0.0, 0.0, if (v > 1.0, 1.0, v)) }
}
```

函数是**其参数的纯标量函数**：不能访问世界/实体/空间、不能写入。可调用其它 `funcs`
与内建。按模块加命名空间；可 `mod.fn(...)` 或模块内裸名调用。

### 4.12 单位（可选、编译期）

```pwe
params { k = 4.0 [1/s^2] }
entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = e; dt = 0.1 [s]
    vx = 0.0 - k * x     # 1/s^2 · m · s = m/s  与 vx 一致
}
```

未标注的值为通配符、永不报错；不一致为 detail 77。

### 4.13 动态槽与循环

* `s[i]` 读 / `s[i] = expr` 写运行时索引处的槽（**仅 `update`**；`rk4` 中被拒，
  detail 73）。
* `repeat n { … }`（≤ 1000）、`for i in lo..hi { … }`（升序整数）、`break`/
  `continue`（`break if (cond)`）——降级期展开（≤ 10000 条语句）。循环体只能含
  `let`、嵌套循环与 `break`/`continue`；槽规则放在循环外。`for` 把索引绑定为局部值。

---

## 5. 呈现与查看器

* 场渲染为两层嵌套等值面（2D/3D）或彩色曲线（1D）。
* 实体按 `shape`/`size`/`color`/`opacity`/`glow`/`label` 渲染。自定义形状支持基本体、
  `hull`、`poly` 网格、SVG 与组合。
* `rotation`（静态）、`orient`（仿真驱动转向/前倾）、`vector=false`（隐藏箭头/环）、
  `label=false`（隐藏名称）。
* 软体绘制网格 bond。
* 查看器控件：**⟳ Restart**（重载初始场景并暂停在 t=0）、**⏸ Pause / ▶ Resume**、
  **🏷 Labels**。

---

## 6. 容易踩的坑（调试前必读）

1. **`slot = expr` 是积分**：即 `slot += dt·expr`。要*赋值*写 `slot = (target - slot)`
   （想精确写入则令 `dt = 1`）。
2. **裸数字右侧是系统参数，不是规则**：`x = 1.0` 设的是参数 `x`（规则为空）。要写
   常量规则用 `x = 0.0 + 1.0`。
3. **作为参数的裸数字须是 `value`**，否则用表达式。`dt = 1/60`、`dt = 0.016` 均可。
4. **自身读取在每个（子）步开始时采样一次**：同一系统内规则同时更新。跨系统时，后跑
   的系统**能看到**先跑系统的写入（read-after-write），故顺序重要（`gravity` 在
   `integrate` 前）。场读取能看到同一步写入；实体读取跨系统时取决于是否同一分量
   （§3.3）。
5. **每个物体选定一种执行模型**（§0.3）。状态槽型物体不响应 `gravity`/`integrate`；
   分量型物体除非也给 `state`，否则忽略 `update`。
6. **`nbody` 的质量在 `state[6]`**，不是 `mass` 字段。
7. **`on = <池>` 展开为全部槽**；`active()` 只对池物体有意义。`spawn` 复制调用者的
   `state`。
8. **保留名**：`let` 不得遮蔽 `t`/`pi`/`e`/`sN`（detail 67）。
9. **`state[7]` 默认是查看器的绕 Z 自转**，除非物体设 `orient = true`（此时 7/8/9 为
   欧拉 pitch/yaw/roll）。步态/相位别放在槽 7，除非你确实要自转。
10. **空间查询只在 `update`/`rk4` 规则及其 `let` 内可用**（`funcs` 中报 detail 70）。
    均为确定性、按 id 排序的扫描。
11. **`fget/fset/flap` 第一个参数是字面量场名**，元数为 3/4 或 4/5。
12. **状态槽最多 16 个**；**循环 ≤ 1000 次迭代 / 10000 条语句**；动态槽左侧仅 `update`
    支持。
13. **`invariant` 在任何写入前使该步失败**——用它挡掉坏状态，而不是“修复”。
14. **`joint`/`soft` 需要 `mass`/`dynamic`**（即 `rigid_body`），且用 `Transform` 位置；
    受关节约束的物体应是分量型。
15. **确定性**：不要依赖哈希表顺序；id 稳定且升序；`random()`/`noise()` 每次运行同种子。

---

## 7. 配方（高质量仿真模式）

**7.1 阻尼受迫谐振子**
```pwe
entity m { state = (x = 1.0, vx = 0.0) }
update { on = m; dt = 0.01
  let k = 12.0 ; let c = 0.4 ; let F = 3.0
  vx = (0.0 - k*x - c*vx + F) + 0.0
  x  = vx
}
```
（注意 `+ 0.0`，使每个右侧都是表达式。）

**7.2 轨道 / N 体**——用 `nbody` 配合 `state = (px,py,pz,vx,vy,vz,m)`（见 `solar.pwe`），
或用显式成对 `update` 实现特殊力。

**7.3 扩散（热）**
```pwe
field heat { width = 16; height = 16; dx = 1.0 }
diffuse { field = heat; rate = 0.2 }
update  { on = probe; dt = 1.0 x = fget(heat, 8, 8) - x }   # 采样场
```

**7.4 波动**——`wave { field = u; prev = um; velocity = v; dt = h; absorb = 0.08; absorb_width = 2 }`
（保持 `c·h/dx ≤ 1/√2`）。

**7.5 粒子（池 + 力 + 查询）**
```pwe
pool p[64] { state = (x=0.0, y=0.0, vx=0.0, vy=0.0); shape=sphere; size=0.1 }
spawn  { on = emitter; pool = p; count = 2; every = 1 }
update { on = p; dt = 0.1
  vx = 0.0 + 2.0*(neighbor_mean(0, 0.5) - x) + 0.0   # 简易内聚
  vy = 0.0 - 9.81*0.1 + 0.0
  x = vx ; y = vy
}
despawn { on = p; when = y < -10.0 }
```

**7.6 链条 / 摆**——每节一个 `distance` `joint`（见 `chain.pwe`）；顶端物体设
`dynamic = false`。

**7.7 软体**——`soft` + `gravity` + `integrate`（§3.5）。

**7.8 事件**——`emit(k, v)`；用 `last_event(k)` 读取；用 `at(T)`、`periodic(P)`、
`schedule(gate,delay,k,v)` 调度。

**7.9 状态机**——用 `when = expr`（如冷却槽）门控写入，或用 `watch` 在零穿越时翻转
模式槽。

**7.10 单位**——给 `state`/参数标注单位；编译器检查一致性（§4.12）。

**7.11 变换与刚体**——分量型（`position`/`velocity`/`mass`），用 `gravity`/`integrate`
驱动；用 `joint` 做连杆。

**7.12 三角网格地面（无缝）**——把采样高度场做成一个 `poly` 形状赋给静态实体；人物用
同一高度函数：
```pwe
shape terrain { part poly = [ (x0,h,z0), … ] faces = [ [i,j,k], … ]; }
entity ground { position=(0,0,0); dynamic=false; shape = terrain; color = 0x6F8F4F; label=false }
```
见 `courtyard.pwe`（网格地面 + 蜿蜒小路 + 转身/前倾的人）。

---

## 8. 诊断

编译失败携带信息、detail code 与源码偏移：

```rust
match pwe_reference::lang::LangRuntime::compile(src) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(src, &e)),
}
```

```text
error 48: system 'update' is missing required parameter 'dt'
  --> line 13, column 9
    |
  13 |         update { on = reactor; dt = 0.0005
    |         ^
```

| Code | 含义 |
| --- | --- |
| 48 | 缺少必需的系统参数。 |
| 49 | 未知系统种类。 |
| 50 | 跨后端不一致（运行时；属 bug，请上报）。 |
| 51 | 凸包至少需要 4 个点。 |
| 52 | 状态槽索引越界（0..=15）。 |
| 53 / 54 | `linear` 缺行 / 行长度不对。 |
| 55 | `funcs` 函数体非法 / `update` 规则为空 / 槽左侧非法。 |
| 56 / 57 / 58 | 表达式 / 数字 / 槽引用解析失败。 |
| 59 | 调用元数不符。 |
| 60 | 程序解析失败。 |
| 62 | 未知的实体或通道名。 |
| 63 | `nbody` 无动态物体。 |
| 64 | 颜色字面量非法。 |
| 65 | 循环次数非法（整数 1..=1000）。 |
| 66 | 循环展开超出语句上限。 |
| 67 | `let` 遮蔽保留名（`t`/`pi`/`e`/`sN`）。 |
| 68 | `for` 区间必须为升序整数。 |
| 69 | 不变式被违反。 |
| 70 | 在规则外使用空间查询。 |
| 71 | `every` 必须是 ≥ 1 的整数。 |
| 72 | `substeps` 必须是 1..=1000 的整数。 |
| 73 | 动态槽左侧仅 `update` 支持（`rk4` 拒绝）。 |
| 75 | `field` 需要 `width`/`height` ≥ 1。 |
| 76 | 导入错误（缺文件、实体/场/通道/池重名）。 |
| 77 | 量纲不一致。 |
| 78 | 未知池名（`spawn`/`despawn`）。 |
| 79 | 未知形状 / 形状引用环。 |

**调试流程**：缩减到一个实体 + 一个系统；核对执行模型（§0.3）；核对 `slot = expr`
的积分语义（§6.1–6.2）；加 `invariant` 捕捉发散；`pwe run … --steps N` 打印状态观察。

---

## 9. 标准库（`std/`）

纯函数模块；常量以参数给出（可用 `--param` 覆盖）。完整签名见 `std/README.md`。

| 模块 | 常量 | 函数（代表） |
| --- | --- | --- |
| `math` | — | `clamp clamp01 lerp mix remap step smoothstep wrap sqr deg rad hypot2 hypot3 min3 max3 sgn deadzone ease_in/out` |
| `forces` | — | `hooke spring_accel damping_accel drag_linear/quadratic_accel coulomb_force gravity_force inverse_square_accel buoyancy_force thrust_accel damper_force` |
| `particles` | — | `terminal_velocity drag_step ballistic_x/y/vy bounce_vy reflect radius_from_mass stopping_distance freefall_time speed` |
| `mechanics` | — | `momentum kinetic_energy reduced_mass elastic_1d_v1/v2 impulse friction_force normal_impulse inertia_rod/disk/sphere torque angular_accel angular_kinetic` |
| `thermal` | `sigma_sb` | `celsius kelvin newton_cooling heat_capacity sensible_heat conduction_flux stefan_boltzmann radiative_cooling thermal_diffusivity thermostat_hysteresis mixing_temp` |
| `acoustics` | `rho_air`、`c_air`、`p_ref` | `speed_of_sound_air wavelength spl pressure_from_spl acoustic_impedance doppler inverse_square sound_intensity beat_frequency` |
| `optics` | `h_planck`、`c_light` | `inverse_square_intensity beer_lambert snell_angle critical_angle fresnel_reflectance reflect_axis wien_peak photon_energy focal_length` |
| `em` | `c_light`、`k_coulomb`、`mu0` | `coulomb_force electric_field potential lorentz_force cyclotron_radius biot_savart_wire poynting plane_wave_b/e impedance_free_space` |
| `chemistry` | `R_gas`、`avogadro` | `atomic_mass element_period/group shell_capacity valence_electrons neutrons mol_from_mass mass_from_mol molarity dilute ideal_pressure/volume arrhenius ph h_from_ph neutralization_volume half_life_decay radioactive_amount` |
| `robotics` | — | `planar2_x/y planar2_ik_q1/q2 pid joint_accel diff_drive_v_left/right trapezoid_peak reach rotate_x/y` |
| `control` | — | `first_order second_order low_pass complementary integrate derivative pid pid_clamped feedforward state_feedback bang_bang hysteresis rate_limit_delta lead within slew` |
| `units` | — | `kmh_to_ms`、`ev_to_j`、`atm_to_pa`、`deg_to_rad`、`g_to_ms2`… |

```pwe
# 路径相对本文件；在 cli/examples/ 中应写 "../../std/…"
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
}
```

---

## 10. 示例（`cli/examples/`）

先编译，再 `run`/`present` `.pweb`。

| 示例 | 展示 |
| --- | --- |
| `bounce.pwe` | `gravity` + `ground_contact`（分量型）。 |
| `heat.pwe` | 2D 场 + 展开的 Gauss–Seidel `flap` 扫描。 |
| `solar.pwe` | `nbody`（state `px,py,pz,vx,vy,vz,m`）+ 轨道显示。 |
| `flock.pwe` | `neighbor_count` / `neighbor_mean` 群集。 |
| `spring/spring.pwe` | 模块 + 参数 + 单位 + `at`/`periodic`。 |
| `domains.pwe` | 组合 `std/forces` + `std/thermal` + `std/em` + `std/chemistry`。 |
| `wave.pwe`、`wave3d.pwe` | 1D / 3D `wave`（3D 含海绵吸收）。 |
| `acoustics.pwe` | 2D 声场 + `std/acoustics` dB。 |
| `robot.pwe`、`humanoid.pwe` | 连杆 / 关节化形象 + 渲染属性。 |
| `shapes.pwe` | 组合形状、多面体、SVG。 |
| `chain.pwe` | 距离关节（摆链）。 |
| `cloth.pwe`、`jelly.pwe` | 软体（薄片、3D 凝胶）。 |
| `particles.pwe` | 池 + `spawn`/`despawn`。 |
| `courtyard.pwe` | 三角网格地面 + 转身/前倾的行人（`orient`）。 |

---

## 附录 A——规范骨架

```pwe
# 1. 导入（路径相对本文件）
import "std/forces"

# 2. world
world {
  title = "…"
  gravity = (0, -9.81, 0)
  params { k = 12.0 }
  entity a { position = (0, 5, 0) velocity = (1, 0, 0) mass = 1.0 sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (40,1,40); color = 0x557755 }
}

# 3. 函数（纯）
funcs { accel(k, x) { 0.0 - k * x } }

# 4. 系统（顺序重要）
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.5 }
  invariant      { on = a; expr = abs(x) < 1e6 }   # 挡掉发散
}
```

## 附录 B——稳定性检查清单

* [ ] 每个物体只用一种模型（§0.3）：分量型**或**状态槽型。
* [ ] 规则是 `slot = <表达式>`（不是裸数字）；需要赋值时用赋值惯用法（§6.1–6.2）。
* [ ] 需要处设置了 `on = <实体|池>`。
* [ ] 系统顺序：力/重力 → 积分 → 约束/边界。
* [ ] `dt` 与求解器稳定条件满足（diffuse/wave）。
* [ ] `invariant` 防止 NaN/发散。
* [ ] 空间查询仅在规则内；场操作第一个参数是字面量场名。
* [ ] 状态 ≤ 16 槽；循环在上限内；动态槽左侧仅 `update`。
* [ ] 除非 `orient = true`，槽 7 视为保留。
* [ ] `random()`/`noise()` 可接受（有种子、确定性）。
