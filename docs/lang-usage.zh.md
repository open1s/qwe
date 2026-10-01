# PWE 语言使用指南

[English](lang-usage.md)

一份完整、自包含的 **PWE** 建模与仿真指南。假设你**此前没有任何** PWE（或仿真
编程）经验。建议先从头读一遍，之后当参考手册用。

**阅读方式**

* **第 0–1 部分（教程）** —— 通过一个又一个可运行的程序来学。新手从这里开始。
* **第 2 部分（参考）** —— 每个关键字、字段、系统、内建、规则。
* **第 3 部分（语义与坑点）**、**第 4 部分（配方）**、**第 5 部分（工具）**、
  **第 6 部分（诊断）**、**第 7 部分（标准库）**、**第 8 部分（示例）**。
* 附录：可复制的**骨架**与**稳定性检查清单**。

本文所有代码块都可直接编译运行。

---

# 第 0 部分 —— 入门

## 0.1 一句话理解“仿真”

仿真就是**一组随时间按规则变化的数字**。这些数字是**状态**（小球的位置与速度、
温度场……）。时间以固定的**步**前进：每一步读取当前状态、计算它如何变化、写回
新状态；步长记为 `dt`（秒）。一条这样的规则就是一个**系统**；**世界**是所有拥有
状态之物。就这些——PWE 只是把它做得精确、快速、可复现。

## 0.2 安装与三条命令

```sh
cargo build --release -p pwe-cli          # 构建 `pwe` 工具（一次即可）

pwe compile scene.pwe -o scene.pweb       # 源码 → 已校验工件
pwe run     scene.pweb --steps 600        # 运行 N 步并打印终态
pwe present scene.pweb --port 8000        # 浏览器实时 3D 查看器
```

`compile` 会带出错行与脱字符提示。`--param K=V`（用于 `run`/`present`）可覆盖模型
参数——便于做实验。

## 0.3 第一个程序：下落的小球

```pwe
world {
  gravity = (0, -9.81, 0)
  entity ball   { position = (0, 5, 0) velocity = (3, 0, 0) sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (20, 1, 20); color = 0x557755 }
}
systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.6 }
}
```

运行：`pwe run ball.pweb --steps 300`，观察到的结尾：

```text
ran 300 steps (interpreter == JIT, every step)
  sim time:  5.000000 s
  #1   ball   pos = ( 14.4000,  0.0000, 0.0000)
  #2   ground pos = (  0.0000, -0.5000, 0.0000)
```

**逐行解释**

* `world { … }` 声明*世界有什么*。
* `gravity = (0, -9.81, 0)` 设定全局重力（向下，y 轴）。
* `entity ball { … }` 声明一个**物体**：`position`/`velocity` 是初始变换/速度；
  `sphere = 0.3` 是半径 0.3 的球形碰撞体；`color` 仅用于渲染。
* `entity ground { … dynamic = false … }` 是**静态**物体（永不动）；`box = (20,1,20)`
  是 20×1×20 的盒碰撞体。
* `systems { … }` 声明*行为*，**每步按顺序**执行：
  * `gravity` 给每个动态物体的 y 速度加 `gravity_y·dt`；
  * `integrate` 给位置加 `velocity·dt`；
  * `ground_contact` 阻止/反射将穿过 `y = 0` 的物体（`restitution = 0.6`，故会
    弹跳并损失能量）。

**你看到的**：小球加速下落、弹跳（每次更低）、最终停在地面，并因初始
`velocity.x = 3` 向右漂移。

## 0.4 动手改（每次只改一处）

* `restitution = 1.0` —— 永远弹（完全弹性）。
* `restitution = 0.0` —— 落地即停。
* `velocity = (6, 2, 0)` —— 更快、且向上。
* 把**两个**系统的 `dt` 都改成 `0.008` —— 物理不变，步更小。

改完重新 `pwe compile … && pwe run …`。这个“改 → 跑 → 看”的循环就是你构建每个
模型的方式。

## 0.5 你现在掌握的五件事

1. **world** = 状态（实体、场、参数）。
2. **entity** = 有状态的物体；id 按声明顺序从 1 起。
3. **systems** = 每步按顺序运行一次的规则。
4. **dt** = 步长；每个系统都用它。
5. **run** = 反复执行系统、每步提交新状态。

## 0.6 最容易踩的一条：物体有两种存储方式

PWE 中物体的动力学数据存在**两套互相独立**的存储里，一个物体选用其一。把数字放
错了地方，就会“什么都不发生”。

| | **分量型物体** | **状态槽型物体** |
| --- | --- | --- |
| 写法 | `position`/`velocity`/`mass`/碰撞体 | `state = (…)` |
| 由谁驱动 | `gravity`、`integrate`、`damping`、`force`、`wall`、`ground_contact` | `update`、`rk4`、`linear`、`nbody` |
| 位置存于 | `Transform.position` | `state[0..2]`（渲染回退） |
| 规则中读写 | `@name.position.x`、`@name.velocity.x` | `x`、`@name.x`、`s0`… |

上面的小球是**分量型**——所以 `gravity`/`integrate` 能推动它。下一课用**状态槽型**，
让你自己写物理。一个物体也可以两者都有，但请**只选一种**，最简单。

---

# 第 1 部分 —— 教程：一步步构建仿真

每课都是一个完整、可运行的程序，并给出预期现象。

## L1 —— 自己写物理：阻尼谐振子

分量系统替你免费提供了牛顿运动。要做别的，就把数字放进 `state`，规则自己写。

```pwe
world {
  gravity = (0, 0, 0)
  entity m { state = (x = 1.0, vx = 0.0) }
}
systems {
  update { on = m; dt = 0.01
    let k = 12.0       # 弹簧常数
    let c = 0.4        # 阻尼
    vx = vx + inte(0.0 - k*x - c*vx)  # vx' = -k·x - c·vx
    x  = x + inte(vx)                 # x'  = vx
  }
}
```

运行：`pwe run osc.pweb --steps 200` → `state = [0.6302, -1.5751]`。质量块来回摆动，
能量缓慢衰减（阻尼），最终趋于 0。

**最关键的一条规则**：`slot = expr` 就是普通的**赋值** —— 每步该槽取这个值。
要*积分*导数，用 `dt` 把步长写出来：

* `vx = vx + inte(0.0 - k*x - c*vx)` ⇒ `vx += dt·(−k·x − c·vx)`（加速度）。
* `x = x + inte(vx)` ⇒ `x += dt·vx`（速度）。

**`inte(E)` 算子**即增量 `dt·E`，故 `x = x + inte(vx)` 与 `x = x + dt*(vx)` 等价；
**`inte slot = rate`** 语句（`integrate`、`+=` 同义）无需写出 `dt` 即完成积分。
**`deriv(E)` 算子**是后向差商 `(E − E_prev)/dt` —— 它做*微分*（首步为 0）。
`rk4` 系统用 `inte slot = rate`，以经典四阶 Runge–Kutta 法积分该导数
（同等 `dt` 下比 Euler 精确得多）。

**递归函数**：函数体可用控制流 `if 条件 { return a } else { return b }`（惰性求值），
因此可递归调用（如 `fact(n) { if n < 1.0 { return 1.0 } else { return n * fact(n - 1.0) } }`）；
而表达式 `if(c,a,b)` 会同时求值两支，不能用于递归。


**赋值 vs 积分**：既然 `=` 是赋值，写常量就是 `slot = target`，无需任何惯用法。

**一个立刻要记住的坑**：`let` 的名字不能是 `t`、`pi`、`e`、`sN`（detail 67）。

**参数 vs 规则**：数值参数按**名称**（针对该系统种类）识别——`update { dt = 0.01 }`
设的是 `dt`，而 `update { x = 1.0 }` 是槽 `x` 的规则（不再需要旧的 `0.0 + 1.0` 花招）。

练习：增大 `k`（更快）；增大 `c`（更快衰减）；给 `vx` 加驱动项 `+ 3.0*sin(2.0*t)`
做成受迫振子。

## L2 —— 参数：不改代码做实验

```pwe
world {
  gravity = (0, 0, 0)
  params { k = 12.0; c = 0.4 }
  entity m { state = (x = 1.0, vx = 0.0) }
}
systems {
  update { on = m; dt = 0.01
    vx = vx + dt*(0.0 - k*x - c*vx)
    x  = x + dt*(vx)
  }
}
```

常量现在放在 `params` 里，规则按名读取。无需重编译即可扫描参数：

```sh
pwe run osc.pweb --steps 200 --param k=40
pwe run osc.pweb --steps 200 --param k=40 --param c=0.0   # 无阻尼
```

参数属于世界状态并写入快照，因此一次运行可由参数完全复现。

## L3 —— 多物体与相互作用

用 `@name.<槽>` **读取另一个物体**：

```pwe
world { gravity = (0, 0, 0)
  entity a { state = (x = 1.0,  vx = 0.0) }
  entity b { state = (x = -1.0, vx = 0.0) }
}
systems {
  update { on = a; dt = 0.01 vx = vx + dt*(0.0 - 8.0*(x - @b.x))   x = x + dt*(vx) }
  update { on = b; dt = 0.01 vx = vx + dt*(0.0 - 8.0*(x - @a.x))   x = x + dt*(vx) }
}
```

两质量块由弹簧相连：反相振荡（等大反向）。`@b.x` 是 b 的槽 `x`；读取在步开始处采样，
故两条规则看到的是对方上一步的值。

**天体间的引力**已内建——`nbody` 约定
`state = (px, py, pz, vx, vy, vz, m)`（质量在**槽 6**，*不是* `mass` 字段）：

```pwe
world { gravity = (0, 0, 0)
  entity sun   { state = (0, 0, 0,  0,    0,    0,  1000) sphere = 1.2 }
  entity earth { state = (4, 0, 0,  0,    15.8, 0,  0.05) sphere = 0.3 }
}
systems { nbody { G = 1.0; dt = 0.001 } }
```

`earth` 绕 `sun` 转：半径保持 ≈ 4（500 步时约在 `(-1.60, 3.65)`，即四分之一圈；整圈
约 1571 步）。这就是缩小版 `solar.pwe`。

## L4 —— 场与偏微分方程（热、波）

`field` 是一张数字网格（连续场基底）。系统 `diffuse`、`poisson`、`wave` 在其上求解
PDE；规则用 `fget`/`fset` 读写单元。

热扩散：在中心播种一次，然后让它扩散（总量守恒）。

```pwe
world { gravity = (0, 0, 0)
  field heat { width = 16; height = 16; dx = 1.0 }
  entity probe { state = (x = 0.0) }
}
systems {
  diffuse { field = heat; rate = 0.2 }          # T += 0.2·∇²T
  update { on = probe; dt = 1.0
    # 在 t=0 播种中心；之后每步写回同值（等于无操作）
    let _ = fset(heat, 8, 8, if(at(0.0), 100.0, fget(heat, 8, 8)))
    x = fget(heat, 8, 8) - x                     # 采样中心温度
  }
}
```

实测：每步 `field heat: total=100.000000`（守恒），中心随热量扩散而降温
`100 → 2.05`（20 步）`→ 0.40`（200 步）。

波是二阶 PDE，用跨两场的蛙跳求解：

```pwe
world { gravity = (0, 0, 0)
  field u  { width = 41; height = 1; dx = 1.0 }   # height=1 ⇒ 一维线
  field um { width = 41; height = 1; dx = 1.0 }   # u(t-dt)
  entity probe { state = (x = 0.0) }
}
systems { wave { field = u; prev = um; velocity = 1.0; dt = 0.5 } }
```

稳定条件很重要：`diffuse` 的 `rate ≤ 1/4`（2D）；`wave` 的 Courant `c·dt/dx ≤ 1/√2`
（2D）。超出就会发散。

## L5 —— 动态实体：粒子系统（池）

实体在编译期固定。要让实体出现/消失，声明一块初始**未激活**的**池**并
`spawn`/`despawn`。

```pwe
world { gravity = (0, 0, 0)
  entity emitter { state = (x = 0.0, y = 0.0, vx = 0.0, vy = 0.0) }
  pool p[64] { state = (x = 0.0, y = 0.0, vx = 0.0, vy = 0.0); shape = sphere; size = 0.1 }
}
systems {
  spawn  { on = emitter; pool = p; count = 2; every = 1 }   # 每步 2 个
  update { on = p; dt = 0.1
    vx = vx + dt*(2.0*(neighbor_mean(0, 0.5) - x))         # 简易内聚
    vy = vy + dt*(0.0 - 9.81*0.1)                                # 重力
    x = x + dt*(vx)
    y = y + dt*(vy)
  }
  despawn { on = p; when = y < -10.0 }                       # 回收
}
```

* `pool p[64]` 生成 64 个未激活槽，命名 `p#0`…`p#63`。
* `spawn` 把**调用者**（`emitter`）的状态复制进最低位的空闲槽。
* `update { on = p; … }` **只在激活槽**上运行（`on = <池>` 展开为全部槽）。
* `despawn` 关闭匹配的槽以便复用。
* `neighbor_count(r)` / `neighbor_mean(slot, r)` / `nearest_dist()` 在这里可用——
  确定性、按 id 排序的扫描。

实测：40 步后 64 个粒子激活，下落并内聚。

## L6 —— 约束：摆链（关节）

关节是分量型物体之间的成对、基于位置的约束。

```pwe
world { gravity = (0, -9.81, 0)
  entity anchor { position = (0, 6, 0) mass = 1.0 dynamic = false }
  entity b1     { position = (0, 5, 0) mass = 1.0 dynamic = true }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  joint { on = anchor; other = b1; type = distance; length = 1.0; iterations = 8 }
}
```

`type = distance` 保持 `|anchor − b1| = 1.0`。`anchor` 的 `dynamic = false` 使其成为
无限质量支点。再加一个 `b2` 与第二个关节即更长链条（见 `chain.pwe`）。其它种类：
`spring`（加 `damping`）、`hinge`/`ball`/`weld`（钉住 `anchor` 点）、`prismatic`
（沿 `axis` 滑动，可选 `limit`）。

## L7 —— 软体（薄片或凝胶）

```pwe
world { gravity = (0, -9.81, 0)
  soft cloth { nx = 6; ny = 6; spacing = 0.4; origin = (-1, 5, 0); mass = 0.1 }
}
systems {
  gravity   { gravity_y = -9.81; dt = 0.016 }
  integrate { dt = 0.016 }
  soft      { body = cloth; stiffness = 1.0; iterations = 6 }
}
```

`soft` 生成 `nx × ny`（× `nz`）粒子网格，粒子间以结构/剪切/弯曲弹簧相连；`soft`
系统做松弛，使薄片在整体形变时保持间距。网格边渲染为 bond。`nz > 1` 即 3D 凝胶
（`jelly.pwe`）。

## L8 —— 事件与调度

```pwe
world { gravity = (0, 0, 0) entity e { state = (x = 0.0) } }
systems {
  update { on = e; dt = 0.1
    let fired = at(0.5)     # 时间窗 [t, t+dt) 覆盖 0.5 的那一步为 1.0
    x = x + dt*(fired)      # 把触发积分进 x
    emit(1.0, x)            # 追加事件（kind=1, payload=x）
  }
}
```

* `emit(kind, payload)` 记录一个有序事件；宿主经 API 读取，规则可用 `last_event(kind)`
  读最近一个。
* `at(T)` 在时钟到达 `T` 时触发一次；`periodic(P[,phase])` 每周期一次；
  `schedule(gate, delay, kind, payload)` 排入未来事件。
* 事件及其队列属于确定性的跨后端契约。

## L9 —— 让它好看（呈现）

呈现永不影响仿真。属性：

```pwe
entity ball { position = (0, 5, 0) sphere = 0.3; color = 0xFF6B4A; opacity = 1.0; glow = 0.3; label = true }
```

* `color = 0xRRGGBB`（恰好 6 位十六进制）、`opacity` `[0,1]`、`glow`（自发光）、`label = false` 隐藏名称、
  `size = v | (dx,dy,dz)` 设定标记/盒尺寸。这些都会校验：`opacity` 超出 `[0,1]` 报 detail 105，
  颜色格式错误报 64；同一个 `entity` / `pool` 体内每个字段**最多出现一次**（重复报 107 ——
  `tag` 例外，可累加 —— 且每个实体最多一个碰撞体 `box` / `sphere` / `hull`）。
* `shape = point | sphere | box | capsule | <自定义>` 覆盖碰撞体形状。
* **自定义形状**可组合基本体、`hull`/`poly` 多面体、SVG，乃至彼此：

  ```pwe
  world {
    shape head { part sphere = 0.115 at (0, 0, 0); }
    shape person {
      part box  = (0.34, 0.46, 0.20) at (0, 0.57, 0);
      part head at (0, 0.92, 0);          # 包含另一个形状
    }
    entity e { state = (x=0,y=0,z=0) shape = person; color = 0x4AC3FF }
  }
  ```

* `rotation = (rx,ry,rz)` 静态倾斜物体（如斜坡）。
* `orient = true` 让**仿真**驱动状态型物体的朝向/前倾：state 槽 7/8/9 为欧拉
  `(pitch, yaw, roll)`。示例（由朝向得 yaw + 前倾）：

  ```pwe
  entity walker { state = (x=0,y=0,z=0, vx=1.0, vz=0.0, dx=1.0, dz=0.0, rx=0.0, ry=0.0, rz=0.0)
                  orient = true; shape = person }
  systems { update { on = walker; dt = 1.0
    ry = (atan2(vx, vz) - ry)     # 面朝行进方向
    rx = (0.12 - rx)              # 前倾
    x = 0.05*vx
    z = 0.05*vz
  } }
  ```

* `vector = false` 隐藏查看器速度箭头 / 轨道环。
* **三角网格、无缝**：把采样高度场做成一个 `poly` 网格作为光滑地面（`courtyard.pwe`）。

## L10 —— 组织：函数、模块、单位

纯辅助函数放在 `funcs`：

```pwe
funcs { accel(k, x) { 0.0 - k * x } }
systems { update { on = m; dt = 0.01 vx = vx + dt*(accel(k, x))   x = x + dt*(vx) } }
```

函数接收参数、返回标量；可调用其他函数与内建，但**无法访问世界**（无实体、无
`@引用`、无空间查询）。

**模块**把模型拆到多文件。模块是一个 `.pwe` 文件（若只定义函数，需含空的
`world { }`）；导入路径**相对当前文件**：

```pwe
# pkg/util.pwe
world { }
funcs { twice(v) { 2.0 * v } }

# main.pwe
import "pkg/util"                      # 在 cli/examples/ 中应写 "../../std/…"
world { gravity = (0,0,0) entity e { state = (x=0.0) } }
systems { update { on = e; dt = 0.1 x = util.twice(1.0) } }
```

**单位**可选，但能在编译期抓错：

```pwe
params { k = 12.0 [1/s^2] }
entity m { state = (x = 1.0 [m], vx = 0.0 [m/s]) }
update { on = m; dt = 0.01 [s] vx = vx + dt*(accel(k, x)) }
```

## L11 —— 精确随机化学动力学（`gillespie`）

有些系统天生不连续：几个分子一次只反应一次。那里的 ODE 描述的是*均值*，不是
事物本身 —— 离散性才是重点。`gillespie` 替你执行精确随机模拟算法（Gillespie SSA）：

```pwe
world {
  gravity = (0, 0, 0)
  params { k = 0.15 [1/s] }
  entity e { state = (A = 40.0, C = 0.0) tag = rx }
}
systems {
  gillespie {
    tag = rx
    dt = 0.1
    events = 8
    channel decay = k * A => (A = A - 1, C = C + 1)
  }
}
```

* 每个带标签实体在状态槽里放**整数物种计数**；`tag` 选反应实体，`dt` 是步长，
  `events`（1..=512，默认 64）是每步反应预算。
* 每条 `channel <名> = <倾向> => (<槽> = <表达式>, …)` 是一个反应。倾向是它当下的
  速率 —— 这里是 `k * A`，即速率 `k·A` 的一级衰变。
* 每步抽取精确等待时间 `τ ~ Exp(a)`，触发落在 `dt` 内的反应，然后停在步边界。
  没有任何欧拉化：一步是对连续时间马尔可夫链的**截断**，不是对它的近似。
* 一个事件的效果**同时**求值 —— 全都读取事件*之前*的状态，所以
  `(A = A - 1, C = C + 1)` 是一次同步迁移；两个效果写同一槽时后者生效。
* 一步需要超过 `events` 个反应时，运行以 **detail 106** 失败，而不是悄悄丢反应：
  调大 `events` 或调小 `dt`。倾向或效果里的拼写错误是**编译错误**（detail 48/52），
  绝不会静默读成 `0.0`。

在许多独立容器上取平均，存活数遵循确定性定律 `A(t) = A0·e^{-kt}`：随机视角与
ODE 视角在总量上一致，在单个分子上不一致。见 `cli/examples/kinetics.pwe` —— 24
个原子一次一跳地排空反应物托盘。

## 常见错误（现象 → 原因 → 修法）

| 现象 | 原因 | 修法 |
| --- | --- | --- |
| 物体完全不动 | 状态型物体却用了 `gravity`/`integrate`（或反之） | 只选一种模型（§0.6） |
| 规则“没效果” | 缺 `on =`（作用于每个物体）或左侧不是有效槽 | 加 `on = <实体\|池>`；核对槽/字段名 |
| 数值总是不变 | `slot = expr` 是**赋值** | 要积分写 `slot = slot + dt*(rate)`（或 `inte slot = rate`） |
| `update` 作用到错误物体 | 没写 `on =` → 作用于**每个**动态物体 | 加 `on = <实体\|池>` |
| 一步内运动怪 | 系统顺序 / 读取时机 | 顺序：力 → 积分 → 约束；读取在步开始 |
| `nbody` 没反应 | 质量不在 `state[6]`（或物体是分量型） | `state = (px,py,pz,vx,vy,vz,m)` |
| 场求解器发散 | 超出 `dt`/稳定条件 | `rate ≤ 1/4`（diffuse 2D）；`c·dt/dx ≤ 1/√2`（wave 2D） |
| 池物体被忽略 | 未激活槽会被跳过 | 先 `spawn`；用 `on = <池>` |
| `let` 编译失败 | 名字遮蔽 `t`/`pi`/`e`/`sN` | 改名 |
| 找不到导入 | 路径相对当前文件 | `import "../../std/forces"` 等 |
| 角度/自转不对 | 槽 7 默认是绕 Z 自转 | 设 `orient = true` 用 7/8/9 作欧拉角 |

## 练习进阶

1. 弹跳小球；调 `restitution`。
2. 谐振子；调 `k`、`c`；加驱动力。
3. 两耦合振子；观察能量交换。
4. 小轨道；改 `G`/速度。
5. 热扩散；改 `rate`；在探针处观察。
6. 弦上的驻波。
7. 用池做粒子喷泉。
8. 用关节做 3 连杆摆。
9. 一块布。
10. 复刻 `courtyard.pwe`：网格地面 + 会转身、前倾的行人。
11. 随机动力学：调 `k`，看 `kinetics.pwe` 一次一跳地排空。

---

# 第 2 部分 —— 语言参考

## 2.1 词法规则

| 词法单元 | 形式 | 说明 |
| --- | --- | --- |
| 注释 | `# …` 或 `// …` | 到行尾；任意位置跳过 |
| `ident` | `[A-Za-z_][A-Za-z0-9_]*` | 实体/槽/参数/函数/形状名 |
| `number` | `-? 数字 ("." 数字)? (("e"\|"E") "-"? 数字)?` | f64 |
| `value` | `number`（`/ number`）? | 字面量**或比值**（`1/60`） |
| `boolean` | `true` \| `false` | |
| `string` | `"…"`（无转义） | 标题、SVG 数据 |
| `color` | `0x` + 6 位十六进制 | `0xRRGGBB` |
| `unit` | `[ m/s^2 ]` | 基本单位 `m kg s A K mol cd`；`* / ^`；仅编译期 |
| `slot` | `s` 数字 | 按位置引用自身状态槽 |
| 常量 | `t`、`pi`、`e` | 时钟（秒）、π、自然常数 |

空白不影响解析；`;` 可选。单位带方括号，以免与 `s[0]` 混淆。

## 2.2 关键字（保留）

* 段 `world` `funcs` `systems`
* world `gravity` `title` `params` `chan` `value` `entity` `field` `pool` `soft`
  `struct` `array` `width` `height` `depth` `dx` `nx` `ny` `nz` `spacing` `origin`
  `shape` `part`
* 实体 `position` `velocity` `state` `vec` `mass` `dynamic` `nbody` `parent`
  `restitution` `friction` `box` `sphere` `hull` `rotation` `camera` `color`
  `size` `opacity` `glow` `label` `orient` `vector`
* 形状 `point` `sphere` `box` `capsule` `svg` `hull` `poly` `at` `depth` `scale`
  `faces`
* 控制 `return` `let` `repeat` `until` `while` `for` `in` `break` `continue` `if`
* 逻辑 `and` `or` `not` `&&` `||` `!` `true` `false`
* 原子 `pi` `e` `t`

系统*种类*与*参数*是普通标识符、在构建期匹配（未知种类为 detail 49）。内建是普通
调用、在降级期特判。

转换：`i64(x)`/`i32(x)`/`u64(x)`/`u32(x)` 向零截断，`f64(x)` 把精确整数加宽
（对浮点是恒等），`bool(x)` 为 `x != 0`。

**整数值（RFC-0043）。** 不带 `.`/指数的数字字面量（`2`、`-7`）是**精确整数**；
带小数的字面量（`2.0`、`1e-3`）是 f64。整数运算精确且截断：`7 / 2 == 3`、
`7 % 3 == 1`、`1 / 2 == 0`。只有**两个操作数都是整数**时表达式才保持整数——只要
出现一个小数操作数，整个表达式就加宽为 f64：`2.5 + 1 == 3.5`、`1.0 / 2.0 == 0.5`。
`let n: i64 = expr` 绑定精确整数（浮点 `expr` 向零截断，非有限值触发 detail 18）；
此后 `n / 2` 仍是整数，而把 `n` 写入状态槽（`slot = n`）会把它加宽为 f64。这是
有意的行为并已记录——依赖 `1 / 2 == 0.5` 的旧程序请改写成 `1.0 / 2.0`。

优先级（高→低）：一元 `-`、`not`/`!` → `* / %` → `+ -` → 比较 → `and` → `or`。

## 2.3 `world` 项

| 项 | 含义 |
| --- | --- |
| `gravity = (x,y,z)` | 全局重力（分量型物体）。 |
| `title = "…"` | 查看器标题。 |
| `params { K = v }` | 模型参数（可用 `--param` 覆盖）。 |
| `chan <name> { value = v }` | 通道实体（`state[0]`）。 |
| `entity <name> { … }` | 一个物体。 |
| `shape <name> { part … }` | 自定义渲染形状。 |
| `struct <name> { field = <默认值> … }` | 具名记录类型（§2.12）。 |
| `field <name> { width; height; dx; depth? }` | 标量网格。 |
| `pool <name>[N] { … }` | N 个未激活槽，用于动态实体。 |
| `soft <name> { nx; ny; nz?; spacing; origin; mass }` | 质量-弹簧网格。 |
| `bond <a> <b> [order=n] [polarity=p] [cloud=true] [min=d] [max=d]` | 两个具名实体之间的渲染键（球棍模型）。 |
| `bonds { tag = <t>; other = <u>?; within = <d>; min = <d>?; axis = (x,y,z)?; angle = <rad>? }` | **邻域键网**：自动连接 `tag = <t>`（给了 `other` 则只连 `t`–`u` 跨组对）且距离落在 `[min, within]` 内的实体对，每帧重算（键随远近形成/断裂，例如化学反应）。`axis`+`angle` 再加**键角**门。 |
| `import "…"` | 模块导入（路径相对当前文件）。 |

`bond` **仅用于呈现**（不是物理约束）：在实体 `<a>` 与 `<b>` 的中心之间画一根
短棍。选项：`order`（1/2/3 = 单/双/三键）、`polarity` ∈ `[0,1]`（把键的颜色从 A
渐变到 B）、`cloud=true`（半透明的共用电子云）、`min`/`max` 距离（两端距离落在
该区间内才画这根键 —— 近距成键/断键，例如化学反应）。

两者的选项都会校验：同一选项**最多出现一次**；`bond` 须满足 `0 ≤ min ≤ max`；
`bonds` 必须给出 `tag` 与 `within`，且 `0 < min ≤ within`。未知键或非数值是错误
（detail 107/108），所以 `withn` 这类拼写错误会立刻报错，而不是悄悄让 `within`
保持无界、把同 tag 的实体不管远近全连起来。

**实体 id 顺序**：已声明实体 `1..E`，然后通道，然后池槽（`<pool>#k`），然后软体粒子
（`<soft>#k`）。

## 2.4 实体字段

| 字段 | 含义 |
| --- | --- |
| `position` / `velocity = (x,y,z)` | 初始变换 / 速度。 |
| `state = (…)` | 状态槽：位置式 `(v0,…)`、命名 `(x=0,…)`、`vec3 pos`（槽 `pos.0…`）、`struct` 类型，或嵌套记录 `(p = (x=0,…))`。最多 16。 |
| `mass` | 质量（nbody、视觉尺寸；joint/soft 必需）。 |
| `dynamic = false` | 静态。 |
| `nbody = false` | 排除出 `nbody`。 |
| `parent = <name>` | 卫星显示。 |
| `restitution` / `friction` | 接触响应。 |
| `box` / `sphere` / `hull` | 碰撞体。 |
| `rotation = (rx,ry,rz)` | 静态欧拉旋转（弧度）。 |
| `camera = true` | 查看器相机。 |

仅呈现：`color`、`shape = point\|sphere\|box\|capsule\|<自定义>`、
`size = v|(dx,dy,dz)`、`opacity`、`glow`、`label = false`、
`orient = true`（state 7/8/9 = 欧拉 pitch/yaw/roll）、
`vector = false`（隐藏箭头/环）。

## 2.5 自定义形状

`part <kind> = <参数> [at (x,y,z)] [scale s]`：

* `sphere = r`、`box = (dx,dy,dz)`、`capsule = (r0, len, r1)`；
* `ring = (radius, tube)`（圆环 —— 例如电子壳层）；
* `cylinder = (radius, height)`、`cone = (radius, height)`、
  `plane = (width, height)`（3D 图元；`plane` 双面渲染）；
* `hull = [(x,y,z), …]`（凸）；
* `poly = [(x,y,z), …] faces = [[i,j,k,…], …]`（任意网格）；
* `svg = "<d>" depth <d>`；
* `part <另形状> [at …] [scale …]`（组合，递归）。

仅呈现的部件选项（全部可选，绝不影响语义）：

* `color = 0xRRGGBB` —— 覆盖该部件的实体颜色（多色形状：质子/中子/电子）；
* `opacity = o` —— 覆盖该部件的实体不透明度（`0..1`）；
* `spin s` —— 自转角速度 rad/s（数值紧跟关键字；`spin = s` 也接受）；
* `orbit (radius, speed, phase)` —— 让部件绕实体局部原点转动（电子绕核），
  `speed` 为 rad/s；`axis (x,y,z)` 设轨道平面法线（默认局部 `+Z`）。

同一部件上每个选项**最多出现一次**（detail 107）；`opacity` 超出 `0..1`
（detail 105）或颜色字面量不合法（detail 64）都是错误，而不是被悄悄丢掉。

## 2.6 网格场

`field f { width=w; height=h; dx=d; depth? }`（2D，或加 `depth` 变 3D）。规则：
`fget(f,i,j[,k])`、`fset(f,i,j,[k,]v)`、`flap(f,i,j[,k])`。第一个参数是字面量场名；
同一一步内更早的写入对之后读取可见。

## 2.7 全部系统种类

`?` = 可选；缺必需 ⇒ detail 48；未知种类 ⇒ detail 49。

| 种类 | 参数 | 模型 | 含义 |
| --- | --- | --- | --- |
| `gravity` | `gravity_y`、`dt` | 分量 | `velocity.y += gravity_y·dt` |
| `integrate` | `dt` | 分量 | `position += velocity·dt` |
| `damping` | `factor` | 分量 | `velocity *= factor` |
| `force` | `ax`,`ay`,`az`,`dt` | 分量 | `velocity += (ax,ay,az)·dt` |
| `wall` | `x`,`z`,`y_min?`,`restitution?` | 分量 | 在 `±x`、`±z` 反射 |
| `ground_contact` | `restitution` | 分量 | 解算 `y=0` 平面 |
| `linear` | `slots`,`dt`,`row0=(…)`,… | 状态 | `ds/dt = A·s + c`（欧拉） |
| `nbody`（velocity-Verlet） | `G`,`dt` | 状态 | 平方反比；`state=(px,py,pz,vx,vy,vz,m)` |
| `pair` | `tag`,`other?`,`dt`,`law`,`coord?`,`cohort?` | 状态 | 通用成对力：对 `tag` 组每个物体，把 `law(r)`（成对距离 `r` 上的力，正值为斥力）对 `other` 组（默认自身）求和；**冲量** `v += a·dt`；`state=(px,py,pz,vx,vy,vz,m)`。`coord = rc` 时律还可使用 `n`（配位数）、`rmin`（最近邻距离）与 `rminj`（对方最近邻距离），于是**对称**键级可表达；`cohort = <tag>` 指定配位数的统计组（默认 `tag` ∪ `other`） |
| `drift` | `tag`,`dt`,`damp?` | 状态 | 带标签物体的位置漂移 `p += v·dt`（可选阻尼 `v *= 1-damp·dt`）；与 `pair` 配对使用 |
| `send`/`recv` | `on`(必需),`chan`,`value` / `on`(必需),`chan`,`slot` | 通道 | 通道收发（限定 `on`） |
| `update` | `dt`,`on?`,`when?`,`every?`,`substeps?`,规则 | 状态 | 显式欧拉 |
| `rk4` | `dt`,`on?`,`when?`,`every?`,`substeps?`,规则 | 状态 | 4 阶 RK |
| `gillespie` | `tag`(必需),`dt`,`events?`,通道 | 状态 | 精确随机化学动力学（Gillespie SSA）：每条 `channel <名> = <倾向> => (<槽> = …, …)` 是一个反应；每步按倾向抽取精确等待时间，最多触发 `events` 个反应（1..=512，默认 64），不够则以 detail 106 失败 |
| `invariant` | `expr`,`on?` | — | 每步断言 |
| `watch` | `expr`,`mem`,`into`,`on?` | 状态 | 零穿越标志 |
| `diffuse` | `field`,`rate` | 场 | `T += rate·∇²T` |
| `poisson` | `field`,`iters`,`source?`,`scale?` | 场 | `∇²φ = ρ·scale` |
| `wave` | `field`,`prev`,`velocity`,`dt`,`damping?`,`absorb?`,`absorb_width?` | 场 | `u_tt = c²∇²u` |
| `spawn` | `on`,`pool`,`count?`,`every?`,`phase?` | 池 | 激活槽 |
| `despawn` | `on`,`when` | 池 | 停用槽 |
| `joint` | `on`,`other`,`type`,… | 变换 | 成对约束 |
| `soft` | `body`,`stiffness?`,`damping?`,`iterations?` | 变换 | 质量-弹簧网格 |

## 2.8 表达式

| 形式 | 含义 |
| --- | --- |
| `s0`、`s1`… | 自身槽 |
| `x`、`@self.x` | 自身命名槽 |
| `@name.sN`、`@name.x`、`@name.state.x` | 另一实体的槽 |
| `@name.mass`、`@name.is_dynamic` | 另一实体的属性 |
| `@name.position.x`、`@name.velocity.x` | 另一实体的变换/速度 |
| `+ - * / %`、`-x` | 算术 |
| `< <= > >= == !=` | 比较 → 1.0/0.0 |
| `and`/`&&`、`or`/`\|\|`、`not`/`!` | 逻辑 |
| `pi`、`e`、`t` | 常量 / 时钟 |

未解析的裸名读到 `0.0`。系统参数不在表达式作用域内（用 `let dt = 0.02` 绑定后使用）。

## 2.9 内建（元数校验；detail 59）

1 元数学：`sin cos exp ln sqrt abs floor ceil round sign log10 log2 sinh cosh tanh
asin acos atan`；2 元：`pow atan2 hypot min max`；`if(c,a,b)`；`random()`、`noise()`；
`print(x)`、`emit(kind,payload)`、`last_event(kind)`；`at(T)`、`periodic(P[,phase])`、
`schedule(gate,delay,kind,payload)`；`active()`；`vlen vdot vdist`；空间
`neighbor_count(r)`、`nearest_dist()`、`neighbor_mean(slot,r)`、`nearest_dx/dy/dz()`；
场 `fget fset flap`。没有 `tan`（用 `sin/cos`）。其它名字是 `funcs` 函数。

## 2.10 `funcs`、单位、循环

* 各段必须按顺序出现：`world`（必需）→ `funcs`（可选）→ `systems`（可选）。

* `funcs { f(a,b) { expr } }` —— 纯标量函数；不能访问世界。
* 单位：给 `state`/参数标注 `[m]`、`[m/s]`、`[1/s^2]`；不一致为 detail 77；未标注即
  通配符。
* `s[i]` 读 / `s[i] = expr` 写（运行时索引；**仅 `update`**）。
* `repeat n {…}`（≤1000）、`for i in lo..hi {…}`（升序）、`break`/`continue`
  （`break if (…)`），降级期展开（≤10000 条语句）。循环体仅含 `let`、嵌套循环、
  `break`/`continue`。

## 2.11 数组（定长、具名）

`array N name [{ v0, … }]` 声明 N 个连续状态槽 `name.0 … name.{N-1}` 并记录长度，
因此 `name[j]` 会被**检查**而不是裸槽下标；可与 `state = (…)` 以任意顺序组合。

```pwe
world {
  gravity = (0, 0, 0)
  entity swarm { state = (x = 0.0) array 4 samples { 1.0, 2.0, 3.0, 4.0 } }
}
systems {
  update { on = swarm; dt = 0.5
    samples[0] = samples[0] + dt*(1.0)   # 常量下标 → 静态槽
    samples[1] += 2.0                    # `+=` 按 dt 积分
    inte samples[2] = 4.0                # `inte name[j] = rate`
    x = samples[x]                        # 运行期下标（x 存 0..3）
  }
}
```

* `name[j]` 读：**常量** `j` 下降为静态槽；**运行期** `j` 下降为运行期索引读
  （`s[base + j]`），与 `s[i]` 一样不做越界检查。
* 写形式：`name[j] = expr`、`name[j] += expr`、`inte name[j] = rate`（读值在系统
  开始时采样，与所有规则一致）。
* 常量 `j` 超出 `[0, N)` 为 **detail 52**；未知数组名为 **detail 109**；初始化值多于
  `N` 也是 detail 52。以上都不会静默读 0.0。
* `vecN pos` 仍是无长度的匿名向量形式；`array` 增加了长度与越界检查。两者都摊平为
  扁平状态槽（零开销）。

* `for` 的边界可以是 `len(name)`——具名数组的声明长度，编译期解析
  （`for j in 0..len(v) { … }`）。未知数组名为 detail 109。**运行期**下标的越界仍
  不检查（与 `s[i]` 相同）。

## 2.12 结构体类型（记录）

`struct` 给一组字段命名；在 `state = …` 中使用它会把这些字段铺成**点分标量槽**：

```pwe
world {
  struct Vec3 { x = 0.0; y = 0.0; z = 0.0 }
  struct Body { pos = Vec3; vel = Vec3; mass = 1.0 }

  entity a { state = Body }                               # 槽 pos.x … mass
  entity b { state = (p = Vec3, hp = 10.0) }              # 嵌入 + 一个标量
  entity c { state = (pos = (x = 7.0, y = 8.0, z = 9.0)) }# 内联记录
}
systems {
  update { on = a; dt = 0.1
    pos.x = pos.x + dt*(vel.x)   # 点分左侧：pos.x += dt·vel.x
    vel.y = vel.y + dt*(0.0 - 9.81)
  }
}
```

* 字段可为标量（带默认值）或其它结构体（可嵌套）。
* 自身字段用 `pos.x`；其它实体用 `@a.pos.x`；规则与 `funcs` 中皆可。
* `struct` 是编译期糖、铺成平坦槽——零成本、确定性、属于世界状态。未知类型与
  引用环报错（detail 80）。
* `vecN name` 是内建简写：`vec3 pos` → `pos.0`、`pos.1`、`pos.2`（`pos` 是 `pos.0` 的别名）。

---

# 第 3 部分 —— 语义与坑点

* **时间是显式的**：每条规则的表达式都乘以 `dt`；`t` 每步前进 `dt`。
* **`slot = expr` 是赋值**。积分用 `slot = slot + inte(rate)`（或 `inte slot = rate` / `+=` 语句）；`inte slot = rate` 在 `rk4` 中以四阶 RK 积分。`inte(E) = dt·E`，`deriv(E) = (E−E_prev)/dt`。
* **语言版本**：`world { lang_version = "0.3" }` 固定语义（v0.3 即上述规则）；
  缺省表示“当前版本”；不支持的值在编译期报错（detail 83）。
  旧版（pre-v0.3）源码可用 `pwe migrate <src.pwe> [-o out.pwe]` 升级
  （旧的隐式 `=` 积分与旧 `deriv` 形式会改写为 `inte`）。
* **系统参数按名称识别**（针对该系统种类）；其它任何 `name = <表达式>`（含 `name = 1.0`）都是规则。
* **读取**：同一系统函数内，所有读取在（子）步开始处采样一次（规则同时）；跨系统时，
  后跑的系统能看到先跑系统的写入——故顺序重要。场读取能看到同一步写入。
* **确定性**：`random()`/`noise()` 有种子、空间扫描按 id 排序、解释器 ≡ JIT
  （`step_cross`）。
* **状态** ≤ 16 槽。**`invariant`** 在任何写入前使该步失败。
* **两种物体模型**（§0.6）。`nbody` 质量在 `state[6]`。除非 `orient = true`，槽 7 为
  绕 Z 自转。
* **池**：未激活槽被跳过并隐藏；`on = <池>` 展开为全部槽。

---

# 第 4 部分 —— 配方（精简）

* **阻尼/受迫振子**：§L1/L2。
* **N 体**：`nbody` 配 `state=(px,py,pz,vx,vy,vz,m)`。
* **特殊成对力**：显式 `update` 读取 `@other`。
* **扩散 / Poisson / 波**：§L4；注意稳定条件。
* **粒子系统**：池 + `spawn`/`despawn` + `neighbor_*`（§L5）。
* **连杆 / 摆**：每节一个 `distance` 关节，顶端 `dynamic=false`（§L6）。
* **布 / 凝胶**：`soft` 配 `nz`（§L7）。
* **事件 / 调度**：`emit`/`last_event`/`at`/`periodic`/`schedule`（§L8）。
* **状态机**：用 `when = expr` 门控写入；用 `watch` 翻转模式。
* **单位与量纲检查**：§L10。
* **三角网格地面（无缝）**：用高度场做一个 `poly` 形状（§L9）。

---

# 第 5 部分 —— 工具

```sh
pwe compile <src.pwe> -o <out.pweb> [--param K=V]
pwe run     <out.pweb> [--steps N] [--param K=V]…
pwe present <out.pweb> [--port P] [--param K=V]…
```

`.pweb` 工件是自描述容器（magic + version），内含已校验 EIR 与模型源码；
`run`/`present` 执行编译后的 EIR。查看器控件：**⟳ Restart**（重载初始场景并暂停在
t=0）、**⏸ Pause / ▶ Resume**、**🏷 Labels**。演示建议用 release 构建。

嵌入 API：

```rust
let mut rt = pwe_reference::lang::LangRuntime::compile(src)?; // 或 ::compile_file
rt.step_cross_n(600)?;                 // 断言 interpreter == JIT
let frame = rt.present_frame(None);    // 渲染快照
```

---

# 第 6 部分 —— 诊断

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
| 80 | 未知结构体类型 / 结构体环 / 状态过大。 |
| 83 | 不支持的 `lang_version`。 |
| 84 | 单位注解格式错误。 |
| 85 | 未知标识符（读作 0.0）—— 警告。 |
| 86 | 求解器设置不稳定（CFL / 扩散极限）。 |
| 87 | 守恒量漂移超过 `tolerance`。 |
| 88 | 状态非有限（仿真发散）—— 配合 `pwe run --check`。 |
| 89 | `let` 类型标注不符。 |
| 90 | 缺少单位注解（`units = "strict"`）。 |
| 91 | `nbody` 物体状态槽不足 7 个。 |
| 92 | `nbody` 物体同时有 `mass` 字段（被忽略；质量为 `state[6]`）—— 警告。 |
| 93 | `rk4` 中使用普通赋值（请用 `inte slot = rate` 积分）。 |
| 94 | 状态槽名与系统参数名冲突 —— 警告。 |
| 95 | `nbody` 物体暴露槽 7/8/9 但未设 `orient = true` —— 警告。 |
| 96 | 规则写入槽 7/8/9 但未设 `orient = true` —— 警告。 |
| 100 | 赋值给未知状态槽（被忽略）—— 警告。 |
| 101 | 模块名重复（两个模块声明了相同的 `module`）。 |
| 102 | 引用了模块未导出的成员。 |
| 103 | 字段名不是小写标识符。 |
| 105 | `opacity` 超出 `0..1`（部件级或 entity 级）。 |
| 106 | `gillespie` 一步超出反应预算（`events`）—— 调大 `events` 或调小 `dt`。 |
| 107 | 重复的选项或字段：部件 / `bond` / `bonds` 的每个选项，以及 `entity` / `pool` 体内除 `tag` 外的每个字段，最多只能出现一次。 |
| 108 | `bond` / `bonds` 选项未知或非法（键拼错、数值不合法、缺 `tag`/`within`、范围颠倒）。 |
| 109 | `name[j]` 中数组名未知（RFC-0044）：该 entity 没有 `name.0 … name.k` 这段状态槽。 |

**调试流程**：缩减到一个实体 + 一个系统；核对模型（§0.6）；核对积分/赋值陷阱；加
`invariant`；`run … --steps N` 读打印状态。

---

# 第 7 部分 —— 标准库（`std/`）

纯函数模块；常量是可覆盖参数。完整签名见 `std/README.md`。

| 模块 | 常量 | 代表函数 |
| --- | --- | --- |
| `math` | — | `clamp clamp01 lerp mix remap step smoothstep wrap sqr deg rad hypot2 hypot3 min3 max3 sgn deadzone ease_in/out` |
| `forces` | — | `hooke spring_accel damping_accel drag_linear/quadratic_accel coulomb_force gravity_force inverse_square_accel buoyancy_force thrust_accel damper_force` |
| `particles` | — | `terminal_velocity drag_step ballistic_x/y/vy bounce_vy reflect radius_from_mass stopping_distance freefall_time speed` |
| `mechanics` | — | `momentum kinetic_energy reduced_mass elastic_1d_v1/v2 impulse friction_force normal_impulse inertia_rod/disk/sphere torque angular_accel angular_kinetic` |
| `thermal` | `sigma_sb` | `celsius kelvin newton_cooling heat_capacity sensible_heat conduction_flux stefan_boltzmann radiative_cooling thermal_diffusivity thermostat_hysteresis mixing_temp` |
| `acoustics` | `rho_air`,`c_air`,`p_ref` | `speed_of_sound_air wavelength spl pressure_from_spl acoustic_impedance doppler inverse_square sound_intensity beat_frequency` |
| `optics` | `h_planck`,`c_light` | `inverse_square_intensity beer_lambert snell_angle critical_angle fresnel_reflectance reflect_axis wien_peak photon_energy focal_length` |
| `em` | `c_light`,`k_coulomb`,`mu0` | `coulomb_force electric_field potential lorentz_force cyclotron_radius biot_savart_wire poynting plane_wave_b/e impedance_free_space` |
| `chemistry` | `R_gas`,`avogadro` | `atomic_mass element_period/group shell_capacity valence_electrons neutrons mol_from_mass mass_from_mol molarity dilute ideal_pressure/volume arrhenius ph h_from_ph neutralization_volume half_life_decay radioactive_amount` |
| `robotics` | — | `planar2_x/y planar2_ik_q1/q2 pid joint_accel diff_drive_v_left/right trapezoid_peak reach rotate_x/y` |
| `control` | — | `first_order second_order low_pass complementary integrate derivative pid pid_clamped feedforward state_feedback bang_bang hysteresis rate_limit_delta lead within slew` |
| `units` | — | `kmh_to_ms ev_to_j atm_to_pa deg_to_rad g_to_ms2 …` |

```pwe
# 在 cli/examples/ 中路径应写 "../../std/…"
import "std/forces"
import "std/thermal"
update { on = body; dt = 0.1
    vx   = vx + dt*(forces.spring_accel(2.0, x, 1.0) + forces.damping_accel(0.3, vx, 1.0))
    temp = temp + dt*(thermal.newton_cooling(temp, 293.15, 0.05))
}
```

---

# 第 8 部分 —— 示例（`cli/examples/`）

先编译，再 `run`/`present` `.pweb`。

| 示例 | 教学点 |
| --- | --- |
| `bounce.pwe` | 分量型：`gravity` + `ground_contact`。 |
| `heat.pwe` | 2D 场 + `flap` 扫描。 |
| `solar.pwe` | `nbody`（state `px,py,pz,vx,vy,vz,m`）。 |
| `flock.pwe` | 群集：`neighbor_count`/`neighbor_mean`。 |
| `spring/spring.pwe` | 模块 + 参数 + 单位 + `at`/`periodic`。 |
| `domains.pwe` | 组合 `std/*`。 |
| `wave.pwe`、`wave3d.pwe` | 1D / 3D 波动求解器。 |
| `acoustics.pwe` | 2D 声场 + dB。 |
| `robot.pwe`、`humanoid.pwe` | 连杆 / 关节化形象。 |
| `shapes.pwe` | 组合形状、多面体、SVG。 |
| `chain.pwe` | 距离关节（摆链）。 |
| `cloth.pwe`、`jelly.pwe` | 软体（薄片、3D 凝胶）。 |
| `particles.pwe` | 池 + `spawn`/`despawn`。 |
| `courtyard.pwe` | 网格地面 + 转身/前倾行人（`orient`）。 |
| `structs.pwe` | `struct` 记录类型（`pos.x`、`@a.pos.y`）。 |
| `kinetics.pwe` | 精确随机动力学：`gillespie` 通道 + SSA。 |

---

## 附录 A —— 规范骨架

```pwe
# 导入（路径相对本文件）
import "std/forces"

world {
  title = "…"
  gravity = (0, -9.81, 0)
  params { k = 12.0 }
  entity a { position = (0, 5, 0) velocity = (1, 0, 0) mass = 1.0 sphere = 0.3; color = 0xFF6B4A }
  entity ground { position = (0, -0.5, 0) dynamic = false; box = (40,1,40); color = 0x557755 }
}

funcs { accel(k, x) { 0.0 - k * x } }

systems {
  gravity        { gravity_y = -9.81; dt = 0.016 }
  integrate      { dt = 0.016 }
  ground_contact { restitution = 0.5 }
  invariant      { on = a; expr = abs(x) < 1e6 }
}
```

## 附录 B —— 稳定性检查清单

* [ ] 每个物体只用一种模型（§0.6）。
* [ ] 规则是 `slot = <表达式>`（**赋值**）；用 `slot = slot + inte(rate)` 或 `inte slot = rate` 积分。
* [ ] 需要处设置了 `on = <实体|池>`。
* [ ] 系统顺序：力/重力 → 积分 → 约束/边界。
* [ ] 求解器稳定条件满足（`diffuse` rate、`wave` Courant）。
* [ ] `invariant` 防止 NaN/发散。
* [ ] 空间查询仅在规则内；场操作第一个参数是字面量场名。
* [ ] 状态 ≤ 16 槽；循环在上限内；动态槽左侧仅 `update`。
* [ ] 除非 `orient = true`，槽 7 保留。
* [ ] `nbody` 物体声明 `state=(px,py,pz,vx,vy,vz,m)`（detail 91）。
* [ ] `random()`/`noise()` 可接受（有种子、确定性）。
