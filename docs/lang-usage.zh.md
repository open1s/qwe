# PWE 语言使用指南

[English](lang-usage.md)

PWE 语言是 `pwe-reference` 的文本前端（`src/lang.rs`，语法文件
`src/lang.pest`）。源程序声明世界模型与系统，编译为低层 EIR，然后**跨后端**
执行——解释器与 CPU JIT 在每一步都必须产生字节级一致的写入。

```
PWE 源码 ──parse──▶ WorldModel + 系统声明 ──lower──▶ EIR ──interpret/JIT──▶ 写入
```

```rust
let compiled = pwe_reference::lang::compile(SOURCE)?;      // 解析 → 降级 → EIR
let mut rt = pwe_reference::lang::LangRuntime::compile(SOURCE)?;
rt.step_cross()?;                                          // interpreter == JIT，每步断言
rt.step_cross_n(30)?;                                      // 一次执行 30 步
```

```sh
pwe compile scene.pwe -o scene.pweb   # .pwe 源码 → 已校验的 .pweb 工件
pwe run     scene.pweb --steps 600    # 运行工件（每步跨后端断言）
pwe present scene.pweb --port 8000    # 浏览器实时 3D 查看器
```

---

# 第一部分 — 语言详解

## 1. 关键字、词法规则与语法

### 1.1 词法规则

| 词法单元 | 形式 | 说明 |
| --- | --- | --- |
| 注释 | `# …` 或 `// …` | 到行尾；在任意位置被跳过 |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | 实体、槽、参数、函数、形状的名字 |
| `number` | `-? 数字 ("." 数字)? (("e"\|"E") "-"? 数字)?` | f64 字面量 |
| `value` | `number` 或 `number "/" number` | 字面量**或比值**（`dt = 1/60`） |
| `boolean` | `true` \| `false` | |
| `string` | `"…"`（无转义） | 标题、SVG 路径数据 |
| `color` | `0x` 十六进制 | `0xRRGGBB` |
| `unit` | `[` 单位 `]` | 基本单位 `m kg s A K mol cd`，算符 `*` `/` `^`——如 `[m/s^2]`、`[1/s]` |
| `slot` | `s` 数字 | 按位置引用自身状态槽（`s0`、`s1`…）；保留 |
| 常量 | `t`、`pi`、`e` | 时钟（秒）、π、自然常数 |

空白不影响解析，语句/参数之间的 `;` **可选**（语法为 `";"?`），因此
`dt = 0.1; x = 1.0` 与 `dt = 0.1` 换行 `x = 1.0` 都能解析。单位必须带方括号，
以免与 `s[0]` 混淆。`let` 不得遮蔽 `t`/`pi`/`e`/`sN`（detail 67）。

### 1.2 关键字

真正的语法 token（不能用作标识符）：

* **段**：`world`、`funcs`、`systems`
* **world**：`gravity`、`title`、`params`、`chan`、`value`、`entity`、`field`、
  `width`、`height`、`depth`、`dx`、`shape`、`part`
* **实体字段**：`position`、`velocity`、`state`、`vec`、`mass`、`dynamic`、
  `nbody`、`parent`、`restitution`、`friction`、`box`、`sphere`、`hull`、
  `camera`、`color`、`size`、`opacity`、`glow`、`label`
* **自定义形状**：`point`、`sphere`、`box`、`capsule`、`svg`、`hull`、`poly`、
  `at`、`depth`、`scale`、`faces`
* **函数 / 控制**：`return`、`let`、`repeat`、`until`、`while`、`for`、`in`、
  `break`、`continue`、`if`
* **逻辑 / 布尔**：`and`、`or`、`not`（以及 `&&`、`||`、`!`）、`true`、`false`
* **原子**：`pi`、`e`、`t`

**上下文名（非保留）**：系统*种类*（`update`、`rk4`、`nbody`、`diffuse`、
`wave`…）与系统*参数*（`on`、`when`、`every`、`substeps`、`dt`、`field`、
`prev`、`velocity`、`rate`、`iters`、`source`、`scale`、`damping`、`absorb`、
`mem`、`into`…）都是普通标识符、在构建期匹配——未知种类报 detail 49。
`import` / `as` / `from` 由模块加载器处理；内建函数名（`sin`、`min`、`if`、
`random`、`emit`…）是普通调用、在降级期特判。

### 1.3 语法（EBNF）

```ebnf
program        = world_section funcs_section? systems_section?

world_section  = "world" "{" world_item* "}"
world_item     = gravity_stmt | title_stmt | params_stmt | chan_stmt
               | entity_stmt | field_stmt | shape_stmt
gravity_stmt   = "gravity" "=" vec3
title_stmt     = "title" "=" string
params_stmt    = "params" "{" (ident "=" value unit? ";")* "}"
chan_stmt      = "chan" ident "{" "value" "=" value ";" "}"
field_stmt     = "field" ident "{" param* "}"

entity_stmt    = "entity" ident "{" entity_field* "}"
entity_field   = position | velocity | state | mass | dynamic | nbody | parent
               | restitution | friction | box | sphere | hull | camera | color
               | shape | size | opacity | glow | label
state_field    = "state" "=" ( "(" state_item ("," state_item)* ")" | vecN )
state_item     = "vec" digits ident | ident "=" value unit? | value unit?
shape_stmt     = "shape" ident "{" shape_part* "}"
shape_part     = "part" shape_kind "=" part_value opt* ";"
part_value     = string | hull_list | vec3 | value
opt            = "at" vec3 | "depth" value | "scale" value | "faces" faces_list
shape_kind     = "point"|"sphere"|"box"|"capsule"|"svg"|"hull"|"poly"

funcs_section  = "funcs" "{" func_def* "}"
func_def       = ident "(" (ident ("," ident)*)? ")" "{" func_body "}"
func_body      = (func_item ";")+ "return" expr | expr
func_item      = let_stmt | repeat_stmt | for_stmt

systems_section= "systems" "{" system* "}"
system         = ident "{" param* "}"
param          = let_stmt | repeat_stmt | for_stmt | slot_lhs "=" expr | call
               | ident "=" ( vecN | expr | ident ) unit? ";"
slot_lhs       = "s" "[" expr "]"
let_stmt       = "let" ident "=" expr
repeat_stmt    = "repeat" number (("until"|"while") "(" expr ")")? "{" loop_item* "}"
for_stmt       = "for" ident "in" number ".." number "{" loop_item* "}"
loop_item      = let_stmt | repeat_stmt | for_stmt | break_stmt | continue_stmt
break_stmt     = "break" ("if" "(" expr ")")?
continue_stmt  = "continue" ("if" "(" expr ")")?

expr           = logical_or
logical_or     = logical_and (("or" |"||") logical_and)*
logical_and    = comparison  (("and"|"&&") comparison)*
comparison     = additive    (("<"|"<="|">"|">="|"=="|"!=") additive)*
additive       = term        (("+"|"-") term)*
term           = factor      (("*"|"/"|"%") factor)*
factor         = unary | number | call | "(" expr ")" | slot | slot_dyn
               | entity_ref | "t" | constant | namespaced | state_name
unary          = "-" factor | ("not"|"!") factor
call           = func_name "(" (expr ("," expr)*)? ")"
func_name      = ident ("." ident)*
slot           = "s" digits
slot_dyn       = "s" "[" expr "]"
entity_ref     = "@" ident "." (slot | prop_path)
prop_path      = ident ("." (ident | digits))*
namespaced     = ident "." ident ("." ident)*

vec3           = "(" value "," value "," value ")"
vecN           = "(" value ("," value)* ")"
hull_list      = "[" vec3 ("," vec3)* "]"
faces_list     = "[" face ("," face)* "]"
face           = "[" digits ("," digits)* "]"
unit           = "[" unit_atom (("*"|"/") unit_atom)* "]"
unit_atom      = ("mol"|"kg"|"cd"|"K"|"A"|"s"|"m") ("^" "-"? digits)? | digits
string         = '"' (any - '"')* '"'
number         = "-"? digits ("." digits)? (("e"|"E") "-"? digits)?
value          = number ("/" number)?
boolean        = "true" | "false"
color          = "0x" hex+
ident          = [A-Za-z_][A-Za-z0-9_]*
```

### 1.4 运算符优先级

由高到低：一元 `-` 与 `not`/`!` → `* / %` → `+ -` → 比较
`< <= > >= == !=` → `and`/`&&` → `or`/`||`。比较与逻辑算符结果为 `1.0`/`0.0`；
非零操作数即为真。`not` 绑定其后的因子——取反比较请写 `not (x > 0)`。

## 2. 程序结构

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

注释从 `#` 或 `//` 到行尾，作为空白在任意位置被跳过（包括规则块内）。其余空白
不影响解析。一个 `.pwe` 文件同时是一个**模块**（见 §4.8）。

## 3. `world` — 世界模型

世界模型描述*世界是什么*（实体、场、参数），不依赖任何 CPU/GPU/OS。

| 语句 | 含义 |
| --- | --- |
| `gravity = (x, y, z)` | 全局均匀重力向量。 |
| `params { G = 1.0; k = 3.0 }` | 运行时可设置的模型参数；规则按名读取，同一工件可用 `--param G=2` 覆盖。 |
| `title = "..."` | 运行标题，`pwe present` 展示。 |
| `entity <name> { fields }` | 一个物体（§3.1）。 |
| `shape <name> { part … }` | 用户自定义**形状**（§3.2）。 |
| `chan <name> { value = v }` | 通道实体；最新值保存在 `state[0]`。 |
| `field <name> { width = w; height = h; dx = d }` | 确定性标量网格场——PDE 基底（§3.3）。 |
| `import "pkg/mod"` | Python 式模块导入（§4.8）。 |

### 3.1 实体字段

实体 id 按声明顺序从 1 开始；通道排在实体之后。

| 字段 | 含义 |
| --- | --- |
| `position = (x, y, z)` | 初始位置。 |
| `velocity = (x, y, z)` | 初始线速度。 |
| `state = (v0, v1, …)` | 通用状态槽（位置式）。每实体最多 16 槽（`s0…s15`）。 |
| `state = (x = 0, y = 0, …)` | 命名状态槽；规则里按名赋值、`@self.x` 读取，可与位置式混用。 |
| `state = (vec3 pos, …)` | 向量元素占 N 个连续槽，命名 `pos`、`pos.0`…`pos.{N-1}`；`pos` 读分量 0、`@self.state.pos.1` 读分量 1、`s[i]` 动态索引。 |
| `mass = v` | 质量（驱动 `nbody`；查看器据此推导视觉尺寸）。 |
| `dynamic = false` | 静态物体（默认动态）。 |
| `nbody = false` | 排除出 `nbody` 系统。 |
| `restitution = v` / `friction = v` | 接触弹性 / 切向摩擦。 |
| `box = (dx,dy,dz)` / `sphere = r` / `hull = [(x,y,z), …]` | 碰撞体（凸包至少 4 点）。 |
| `camera = true` | 标记为查看器相机（不参与仿真）。 |

**仅呈现字段**（不影响仿真状态、确定性或状态哈希）：

| 字段 | 含义 |
| --- | --- |
| `color = 0xRRGGBB` | 呈现颜色。 |
| `shape = point \| sphere \| box \| capsule \| <自定义>` | 显示形状（覆盖碰撞体推导；`<自定义>` 引用 §3.2 的形状）。 |
| `size = v \| (dx,dy,dz)` | 标记直径 / 球半径 / 盒边长，或按轴盒尺寸；对自定义形状则整体缩放。 |
| `opacity = v` | 不透明度 `[0, 1]`。 |
| `glow = v` | 自发光强度（0=哑光）。 |
| `label = false` | 隐藏浮动名称标签（默认 `true`）。 |

### 3.2 自定义形状（基本体、多面体、SVG）

world 可声明具名**自定义形状**，实体按名引用。形状是一组带偏移的部件。

```pwe
world {
  shape drone {                       # 组合基本体
    part capsule = (0.06, 0.30, 0.06);
    part sphere  = 0.09 at (0, 0.20, 0);
    part box     = (0.54, 0.02, 0.02) at (0, 0.20, 0);
  }
  shape octa {                        # 由顶点定义的凸多面体
    part hull = [(0,0.9,0), (0.9,0,0), (0,-0.9,0), (-0.9,0,0), (0,0,0.9), (0,0,-0.9)];
  }
  shape gem {                         # 任意多面体：顶点 + 面
    part poly = [(0,0.9,0), (0.7,0,0.7), (-0.7,0,0.7), (-0.7,0,-0.7), (0.7,0,-0.7), (0,-0.9,0)]
      faces = [[0,1,2],[0,2,3],[0,3,4],[0,4,1],[5,1,4],[5,4,3],[5,3,2],[5,2,1]];
  }
  shape star {                        # 挤出的 SVG 路径
    part svg = "M 0,-1 L 0.224,-0.309 L 0.951,-0.309 L 0.363,0.118 L 0.588,0.809 L 0,0.382 L -0.588,0.809 L -0.363,0.118 L -0.951,-0.309 Z" depth 0.22 scale 0.8;
  }
  entity craft { state = (x = 0.0, y = 0.0, z = 0.0) shape = drone; color = 0x4AC3FF }
}
```

`part <kind> = <params> [at (x,y,z)] [scale s]`：

* `sphere = r`、`box = (dx,dy,dz)`、`capsule = (底半径, 长度, 顶半径)`——
  球 / 盒 / 平滑（可渐变）回转体；
* `hull = [(x,y,z), …]`——由顶点定义的**凸多面体**；
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]`——显式顶点 + 面索引的**任意
  多面体**（可非凸；查看器对面做三角化）；
* `svg = "<path d>" depth <d>`——SVG 路径沿 Z 挤出。

`at` 在实体局部坐标系中偏移该部件；`scale` 设定该部件尺寸（幅度）。实体可用
`size = s` 缩放整个形状。自定义形状**仅用于呈现**。

### 3.3 网格场——4D 连续场基底

`field <name> { width = w; height = h; dx = d }`（2D）或加 `depth = d`（3D）。
单元是确定性世界状态（可快照/重放）。空间是 3D——加上仿真时钟，场即 4D 基底
（3D 空间 + 时间）。

* `fget(f, i, j)` / `fget(f, i, j, k)`——单元值；能看到同一步内的写入。
* `fset(f, i, j, v)` / `fset(f, i, j, k, v)`——写单元（裸调用语句）。
* `flap(f, i, j)` / `flap(f, i, j, k)`——零通量离散拉普拉斯，按 `1/dx²` 缩放
  （2D 五点、3D 七点）。
* 未知场名读到 `0`（未解析引用约定）。

### 3.4 实体池——动态实体（RFC-0038）

`pool` 是一块**预分配**的实体槽：初始全部**未激活**，运行时才启用/停用，从而保持
EIR 静态不变（每个实体一个函数）：

```
world {
  entity emitter { state = (x = -6.0, vx = 1.5) }
  pool p[24] { state = (x = 0.0, vx = 0.0) shape = sphere size = 0.18 }
}
systems {
  spawn   { on = emitter; pool = p }          # 每步激活一个空闲槽
  update  { on = p; dt = 0.1 x = 0.0 + vx }   # 仅在已激活槽上运行
  despawn { on = p; when = x > 6.0 }          # 越过边界后回收
}
```

* 槽命名为 `<pool>#0`、`<pool>#1`…；其 id 排在已声明实体与通道之后。
* 初始全部未激活；未激活槽被用户系统跳过（其每实体函数以 `active()` 守卫开头），
  并在渲染视图中隐藏；`despawn` 仍会在其上运行以清除标志。
* `spawn` 激活 id 最小的若干空闲槽并复制调用者的 state：`count = n` 每步发射最多
  `n` 个槽（批量），`every = k` / `phase = m` 限定在 `step % k == m` 的步发射
  （带相位）。`active()` 读取当前实体的激活标志（0/1）。
* `active` 属于世界状态：参与哈希与快照（旧快照恢复后全部实体视为激活）。整个过程
  确定性、解释器与 JIT 逐字节一致。

## 4. 系统——行为

### 4.1 内建系统种类

| 系统 | 参数 | 含义 |
| --- | --- | --- |
| `gravity` | `gravity_y`, `dt` | 对动态物体施加重力。 |
| `integrate` | `dt` | 速度积分为位置。 |
| `damping` | `factor` | 每步缩放速度。 |
| `ground_contact` | `restitution` | 解算与地面接触。 |
| `wall` | `x`, `z`, `y_min?`, `restitution?` | 有界域；撞墙反射。 |
| `force` | `ax`, `ay`, `az`, `dt` | 恒定加速度。 |
| `linear` | `slots`, `dt`, `row0 = (a0, …, c)` | 线性系统 `s_N' = Σ a_j·s_j + c`；每行 `slots+1` 项。 |
| `nbody` | `G`, `dt` | 动态物体间平方反比力（`G>0` 引力，`G<0` 斥力）。 |
| `send` / `recv` | `chan`, `value` / `slot` | Go 式通道收发。 |
| `update` / `rk4` | 见 §4.2 | 用户自定义 ODE 规则（欧拉 / RK4）。 |
| `invariant` | `on?`, `expr`, `let …` | 每步断言（§4.3）。 |
| `watch` | `on?`, `expr`, `mem`, `into` | 零穿越检测（§4.4）。 |
| `diffuse` | `field`, `rate` | 显式扩散 `T += rate·∇²T`（Jacobi，严格守恒）。 |
| `poisson` | `field`, `source?`, `iters`, `scale?` | 对 `∇²φ = ρ·scale` 做 Gauss–Seidel 松弛。 |
| `wave` | `field`, `prev`, `velocity`, `dt`, `damping?`, `absorb?`, `absorb_width?` | 二阶蛙跳波动方程（§4.5）。 |

未知系统种类报错（detail 49）。

### 4.2 `update` / `rk4`

两者都接受 `slot = expr`。`update` 含义是 **`slot += dt · expr(state)`**（显式
欧拉）；`rk4` 每步把同一导数求 4 次再合并（4 阶精度）。所有读取先发生——自身槽、
跨实体引用、属性每步读一次，因此同一步内规则**同时更新**。

```pwe
systems {
    update { on = target; dt = 0.02
        let omega = 0.5
        tx = -@self.ty * omega        # tx' = -ty·ω（圆周运动）
        ty = @self.tx * omega
    }
}
```

* `on = <name>`：限定单一实体（默认所有动态物体）。
* `let name = expr`：规则前先算的可复用局部值。
* `when = expr`：门控所有写入（为 0 时状态不动）——状态机语义。
* `every = n`：仅当 `step % n == 0` 运行。
* `substeps = n`：以 `dt/n` 重复积分 n 次。
* 左侧为 `sN` 或实体 `state = (…)` 布局中的命名槽。
* 注意 `slot = expr` 是**积分**；要"赋值"用 `slot = (target - slot)`（于是
  `slot += dt·(target-slot) = target`）。

### 4.3 `invariant`——断言

在**系统跑完后**逐实体求值，必须非零；为零（或 NaN）时该步失败（detail 69），
且在任何写入应用之前失败——场景保持本步之前的状态。

```pwe
invariant { on = reactor; expr = abs((na + naoh) - @self.state.total) < 0.001 }
```

### 4.4 `watch`——零穿越检测

每步把 `expr` 与上一步的值（存于 `mem` 状态槽——持久世界状态）比较，向 `into`
写 0/1 标志：严格变号时为 1。实体自身规则读取该标志并反应（反弹、切换模式）。

```pwe
watch { on = ball; expr = x; mem = 5; into = 6 }
```

### 4.5 连续场求解器（`diffuse` / `poisson` / `wave`）

```pwe
systems {
  diffuse { field = heat; rate = 0.2 }                          # T += 0.2·∇²T
  poisson { field = phi; source = rho; iters = 20 }             # ∇²φ = ρ
  wave    { field = u; prev = um; velocity = 1.0; dt = 0.5 }    # u_tt = c²∇²u
}
```

* `diffuse`——每步一次 **Jacobi** 扫描；零通量模板下总量严格守恒。稳定条件
  `rate ≤ 1/4`（2D）/ `≤ 1/6`（3D）。
* `poisson`——每步 `iters` 次就地 Gauss–Seidel 扫描；边界单元为固定电势。
* `wave`——跨两个场的蛙跳（`prev` = `u(t−h)`）；Courant `c·h/dx ≤ 1/√2`（2D）/
  `≤ 1/√3`（3D）。`damping`（默认 `1.0`，无损）缩放时间项；`absorb` +
  `absorb_width` 在边界加渐变海绵层，吸收外传波而非反射。
* `depth > 1` 时按 3D 迭代；每步只跑一次；确定性；降级为既有场指令
  （解释器 = JIT）。

### 4.6 表达式

| 形式 | 含义 |
| --- | --- |
| `s0`, `s1`, … | 自身状态槽。 |
| `x`（命名槽）、`@self.x` | 自身命名状态槽。 |
| `@name.sN`, `@name.state.x`, `@name.x` | 另一实体的状态槽。 |
| `@name.mass`, `@name.is_dynamic` | 另一实体的属性。 |
| `@name.position.x/y/z`, `@name.velocity.x/y/z` | 另一实体的变换/速度。 |
| `+ - * / %`、一元 `-` | 算术（f64）。 |
| `< <= > >= == !=` | 比较 → `1.0` / `0.0`。 |
| `and`/`&&`、`or`/`\|\|`、`not`/`!` | 逻辑连接词（非零为真）。优先级：`not` > `and` > `or` > 比较。 |
| `pi`, `e` | 常数。 |
| `t` | 全局仿真时钟（秒）。 |

裸名字若既非局部值、槽，也非参数则读到 `0.0`（未解析引用约定）。系统参数（如
`dt`）**不在表达式作用域内**。

**内建函数**——1 元：`sin cos exp ln sqrt abs floor ceil round sign log10 log2
sinh cosh tanh asin acos atan`；2 元：`pow atan2 hypot min max`；另有
`if(c,a,b)`、`random()`（有种子）、`noise()`（有种子正态）、`print(x)`、
`emit(kind,payload)`、`last_event(kind)`。向量助手：`vlen`、`vdot`、`vdist`。
空间查询：`neighbor_count(r)`、`nearest_dist()`、`neighbor_mean(slot,r)`、
`nearest_dx/dy/dz()`（仅规则内）。

**计划事件**：`at(T)`——时间窗 `[t,t+dt)` 覆盖 `T` 的那一步为 1.0；
`periodic(P[,phase])`——每周期一次。

**单位**为可选、编译期检查（未标注为通配符、永不报错）：

```pwe
params { k = 4.0 [1/s^2] }
entity e { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = e; dt = 0.1 [s]
    vx = 0.0 - k * x     # 1/s^2 · m · s = m/s  与 vx 一致
}
```

**动态槽**：`s[i]` 读 / `s[i] = expr` 写运行时索引处的槽（仅 `update`；`rk4`
中被拒，detail 73）。**循环**：`repeat n { … }`、`for i in lo..hi { … }`、
`break`/`continue`——降级期展开（≤1000 次迭代、≤10000 条语句）；循环体只能有
`let`、嵌套循环与 `break`/`continue`。

### 4.7 内建（内在）函数——完整参考

元数在编译期校验（不符 → detail 59）。未列出的调用名是用户自定义函数，从
`funcs` 解析（存在性与元数在降级期校验）。**没有 `tan`**——请写 `sin(x)/cos(x)`。

**初等数学**

| 调用 | 结果 |
| --- | --- |
| `sin(x)`、`cos(x)` | 三角（弧度） |
| `tanh(x)`、`sinh(x)`、`cosh(x)` | 双曲 |
| `asin(x)`、`acos(x)`、`atan(x)` | 反三角（弧度） |
| `atan2(y, x)` | 点 `(x, y)` 的辐角，范围 `(-π, π]` |
| `exp(x)` | eˣ |
| `ln(x)`、`log10(x)`、`log2(x)` | 自然 / 以 10 / 以 2 为底的对数 |
| `sqrt(x)` | √x（负数为 NaN；单位指数减半） |
| `pow(a, b)` | aᵇ |
| `hypot(a, b)` | √(a²+b²) |
| `abs(x)` | 绝对值 |
| `floor(x)`、`ceil(x)`、`round(x)` | 向下 / 向上 / 远离零取整 |
| `sign(x)` | −1、0 或 1 |

**选择**

| 调用 | 结果 |
| --- | --- |
| `if(c, a, b)` | `c ≠ 0` 取 `a`，否则取 `b`（两支都会求值，未用的一支被丢弃） |
| `min(a, b)`、`max(a, b)` | 两值中的较小 / 较大者 |

**随机（有种子、可复放）**

| 调用 | 结果 |
| --- | --- |
| `random()` | `[0, 1)` 均匀抽取 |
| `noise()` | 标准正态（Box–Muller，两次抽取）；始终有限 |

**I/O 与事件**

| 调用 | 作用 |
| --- | --- |
| `print(x)` | 记录 `x` 并原样返回（不改变世界状态） |
| `emit(kind, payload)` | 追加一个有序事件 `(kind, payload)`，返回 `0.0` |
| `last_event(kind)` | 本步已发出的该 `kind` 最近事件的 payload（无则 `0`）。事件每步清空；宿主经 `emitted_events()` 读取。`kind` 是数值而非位模式。 |

**调度（步网格上精确一次）**

| 调用 | 触发 |
| --- | --- |
| `at(T)` | 时间窗 `[t, t+dt)` 覆盖 `T` 的那一步为 `1.0`，否则 `0.0` |
| `periodic(P[, phase])` | 每 `P` 秒一次为 `1.0`（可选相位偏移）；要求 `P > dt` |
| `schedule(gate, delay, kind, payload)` | `gate ≠ 0` 时把 `(kind, payload)` 入队，`delay` 秒后触发——动态事件队列，确定性排空（属跨后端契约） |

**空间查询**（仅在系统规则及其 `let` 块内有效；函数体内报 detail 70）

| 调用 | 结果 |
| --- | --- |
| `neighbor_count(r)` | 自身 `r` 距离内其他物体个数 |
| `nearest_dist()` | 到最近其他物体的距离（独存为 `f64::MAX`） |
| `neighbor_mean(slot, r)` | `r` 内邻居的状态槽 `slot` 均值（无则 0） |
| `nearest_dx()`、`nearest_dy()`、`nearest_dz()` | 各轴 `(最近 − 自身)` 偏移（独存为 0） |

均为确定性（按 id 排序扫描）。位置取 `Transform`，否则 `state[0..2]`。

**向量助手**（对标量分量的纯算术）

| 调用 | 结果 |
| --- | --- |
| `vlen(x, y, z)` | √(x²+y²+z²) |
| `vdot(x1,y1,z1, x2,y2,z2)` | x1·x2 + y1·y2 + z1·z2 |
| `vdist(x1,y1,z1, x2,y2,z2)` | 两点距离 |

**网格场访问**——第一个参数必须是字面量场名（2D 或 3D）

| 调用 | 作用 |
| --- | --- |
| `fget(f, i, j)` / `fget(f, i, j, k)` | 读单元（能看到同一步内写入） |
| `fset(f, i, j, v)` / `fset(f, i, j, k, v)` | 写单元（也可作裸语句；返回 `0.0`） |
| `flap(f, i, j)` / `flap(f, i, j, k)` | 离散拉普拉斯（零通量，按 `1/dx²` 缩放） |

### 4.8 模块与包

每个 `.pwe` 文件是一个模块。`import` 遵循 Python：

```pwe
import "physics"                 # physics.G、physics.thrust(m)
import "physics" as ph           # ph.G
from "physics" import thrust     # thrust(m)（裸名）
```

**包**即目录（`import "shapes"` → `shapes/__init__.pwe`）。函数与参数按模块加
命名空间；模块自身规则先在其命名空间内解析裸名、再回退全局。实体 / 系统 / 场 /
通道扁平合并（重名报错，detail 76）。循环导入可解析；成员按每个别名注册。

## 5. 标准库（`std/`）

纯函数模块：`math`、`particles`、`forces`、`mechanics`、`chemistry`（元素周期表
1–118）、`thermal`、`acoustics`、`optics`、`em`、`robotics`、`units`、`control`。
物理常量以参数给出（`chemistry.R_gas`、`thermal.sigma_sb`、`em.k_coulomb` 等），
可用 `--param` 覆盖。完整 API 见 `std/README.md`。

```pwe
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0) + 0.0
    temp = thermal.newton_cooling(temp, 293.15, 0.05) + 0.0
}
```

---

# 第二部分 — 用法

```sh
pwe compile <src.pwe> -o <out.pweb> [--param K=V]   # 解析 → 校验 → .pweb
pwe run     <out.pweb> [--steps N] [--param K=V]... # 运行工件
pwe present <out.pweb> [--port P] [--param K=V]...  # 实时 3D 查看器
```

* 工件是自描述容器（magic + version），内含已校验的规范 EIR 与模型源码；
  `run`/`present` 执行编译后的 EIR。编译错误会带脱字符渲染出错源码行。
* `--param K=V` 覆盖已声明的模型参数（校验合法；别名一起更新）。
* 实时查看器轮询运行时并提供 **⟳ Restart**（重载初始场景并暂停在 t=0）、
  **⏸ Pause / ▶ Resume**、**🏷 Labels**（整体显示/隐藏名称）。场渲染为等值面
  （2D/3D）或彩色曲线（1D）；实体按其 `shape`/`size`/`color`/`opacity`/`glow`/
  `label` 渲染。
* 演示建议用 release：`cargo build --release -p pwe-cli`。

---

# 第三部分 — 示例：内容、运行、效果

均在 `cli/examples/` 下。先编译，再 `run`/`present` `.pweb`。

| 示例 | 展示 | 运行 | 效果 |
| --- | --- | --- | --- |
| `bounce.pwe` | `gravity` + `ground_contact` | `pwe run bounce.pweb --steps 300` | 小球下落、按弹性反弹；探针状态振荡。 |
| `heat.pwe` | 2D 网格场 + 展开的 Gauss–Seidel `flap` 扫描 | `pwe run heat.pweb --steps 400` | 中心恒热扩散成稳态径向分布；中心温度趋于稳定。 |
| `solar.pwe` | `nbody` + `update`（太阳 + 8 行星 + 月球） | `pwe present solar.pweb` | 发光太阳 + 绕行行星、轨道环、逐体图例。 |
| `flock.pwe` | `neighbor_count` / `neighbor_mean` | `pwe present flock.pweb` | 大量个体聚合并对齐成鸟群。 |
| `spring/spring.pwe` | 模块 + 参数 + 单位 + `at`/`periodic` | `pwe run spring.pweb --steps 400 --param k=16` | 受周期脉冲驱动的阻尼弹簧；改 `k` 改变频率。 |
| `domains.pwe` | 组合 `std/forces`+`std/thermal`+`std/em`+`std/chemistry` | `pwe run domains.pweb --steps 200` | 阻尼弹簧上的探针同时向环境辐射降温。 |
| `wave.pwe` | 1D `wave` 求解器、正弦驻波 | `pwe run wave.pweb --steps 40` | 探针在 −1 与 +1 间摆动（总量守恒为 0）；`present` 画出能量着色的正弦曲线。 |
| `wave3d.pwe` | 3D `wave` + 海绵吸收 | `pwe run wave3d.pweb --steps 60` | 每个脉冲使总量升到 1 再衰减；`present` 显示半透明蓝色球面壳向外扩散并衰减。 |
| `acoustics.pwe` | 2D 声场 + `std/acoustics` dB | `pwe run acoustics.pweb --steps 80` | 驱动单极子辐射；不同距离探针记录到达延迟与 dB 电平。 |
| `robot.pwe` | 2 连杆臂、`std/robotics` 正运动学、渲染属性 | `pwe present robot.pweb` | 摆动的 2 连杆臂：细长盒连杆、球关节、红色末端工具。 |
| `humanoid.pwe` | 62 部件：胶囊四肢、面部细节、手指 | `pwe present humanoid.pweb` | 一个行走的类人形象；默认关闭标签（按 🏷 显示）。 |
| `shapes.pwe` | 自定义形状：组合体、多面体、SVG | `pwe present shapes.pweb` | 无人机（基本体）、挤出 SVG 星形、凸八面体、显式面宝石。 |

快速上手：

```sh
cargo build --release -p pwe-cli
./target/release/pwe compile cli/examples/wave3d.pwe -o wave3d.pweb
./target/release/pwe present wave3d.pweb --port 8000   # 先 ⟳ Restart，再 ▶ Resume
```

```
# 观察：field u total=1.000000（n=3），随后被海绵吸收而衰减
$ ./target/release/pwe run wave3d.pweb --steps 60
```

---

# 第四部分 — 诊断与错误码

编译失败携带人类可读信息、detail code 与源码偏移：

```rust
match pwe_reference::lang::LangRuntime::compile(source) {
    Ok(_) => {}
    Err(e) => eprintln!("{}", pwe_reference::lang::diagnose(source, &e)),
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
| 51 | 凸包至少需要 4 个点。 |
| 52 | 状态槽数量超出范围（1..=16）。 |
| 53 / 54 | `linear` 缺少行 / 行长度不对。 |
| 55 | `funcs` 函数体非法 / `update` 规则为空 / 槽左侧非法。 |
| 56 / 57 / 58 | 表达式 / 数字 / 槽引用解析失败。 |
| 59 | 调用元数不符。 |
| 60 | 程序解析失败。 |
| 62 | 未知的实体或通道名。 |
| 64 | 颜色字面量非法。 |
| 69 | 不变式被违反。 |
| 70 | 在规则外使用空间查询。 |
| 73 | `rk4` 中的动态槽左侧。 |
| 76 | 导入/模块错误（缺文件、实体/场重名）。 |
| 77 | 量纲不一致。 |

---

# 第五部分 — 语义注意

* **时间是显式的**：`dt` 乘以每条规则的表达式；时钟 `t` 每步前进 `dt`。
* **先读后写**：同一步内，每个被引用的值都是这一步**开始时**的值。
* **确定性是一等公民**：`random()` 有种子、可复放；解释器与 JIT 每步字节级一致
  （`step_cross`）。
* **状态槽**每实体上限 16 个。
* **通用基底**：因为规则是作用在命名状态槽上的普通标量 ODE，配合局部值、函数、
  跨实体引用、`random()`/`emit()`，以及 RK4/欧拉积分，语言并不局限于刚体物理——
  任何（耦合）微分或差分方程都能建模、仿真与可视化。
