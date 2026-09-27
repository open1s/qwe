# Quick start

Requirements: **Rust 1.81+** and a browser for the live viewer. `pwe-api` is
`no_std` and dependency-free; `pwe-reference` depends only on `pwe-api` and
PEST.

## Build and test

```sh
git clone https://github.com/open1s/qwe.git && cd qwe

cargo build --workspace
cargo test  --workspace
cargo run -p pwe-conformance     # RFC-0029: all PASS, no skips
```

The `no_std` gate:

```sh
cargo check -p pwe-api --no-default-features
```

## The 60-second tour

```sh
cargo build --release -p pwe-cli

# A person-like figure walks (62-part humanoid: capsule limbs, eyes, fingers)
./target/release/pwe compile cli/examples/humanoid.pwe -o humanoid.pweb
./target/release/pwe present humanoid.pweb --port 8000   # open http://localhost:8000

# A pulse radiates through a 3-D cube — ⟳ Restart, then ▶ Resume
./target/release/pwe compile cli/examples/wave3d.pwe -o wave3d.pweb
./target/release/pwe present wave3d.pweb --port 8000

# A whole solar system, live
./target/release/pwe compile cli/examples/solar.pwe -o solar.pweb
./target/release/pwe present solar.pweb --port 8000
```

## The toolchain

```sh
pwe compile <src.pwe> [-o <out.pweb>]           # source → verified .pweb artifact
pwe run     <out.pweb> [--steps N] [--param K=V]...
pwe present <out.pweb> [--port P] [--param K=V]...
```

Exit codes: `0` success, `1` compile/runtime failure, `2` usage error.

Every artifact is a **self-describing, verified binary** (`.pweb`): recompile,
rerun, restage — the same bytes, the same world. `run` and `present` execute the
compiled EIR the way `java` runs a `.class`; a source must be compiled first.

## Hello, world(model)

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

```sh
pwe compile bounce.pwe -o bounce.pweb
pwe present bounce.pweb --port 8000
```

Viewer controls: **⟳ Restart**, **⏸ Pause / ▶ Resume**, **🏷 Labels**.

## What to read next

* [Language guide & reference](../lang-usage.md) — Part 0 is a guided first
  lesson, Part 1 a tutorial, Part 2 the full reference.
* [Example worlds](examples.md) — the showcase demos.
* [Standard library](../../std/README.md) — what you can import.
* [FAQ](faq.md) — the questions people ask first.
