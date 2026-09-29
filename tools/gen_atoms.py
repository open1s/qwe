#!/usr/bin/env python3
"""Generate `std/atoms/<Sym>.pwe`: a schematic, renderable atomic structure.

Each module declares a `shape <Sym>_atom` (import it, then set it on an entity)
built from the Layer-A primitives:

* the **nucleus** — up to 12 protons (red) + neutrons (grey) on a small sphere,
  proportional to the real nucleon mix (heavier nuclei are drawn schematic);
* one **shell ring** (`ring`) per occupied shell;
* **electrons** — up to 4 per shell (schematic) as small blue spheres with an
  `orbit (radius, speed, phase)` animation, inner shells orbiting faster.

    python3 tools/gen_atoms.py path/to/PeriodicTableJSON.json

Numeric data lives in `std/elements/`; this module is presentation only.
"""
import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
P_COLOR, N_COLOR, E_COLOR, RING_COLOR = 0xFF5555, 0x9AA0A6, 0x4EA1FF, 0x7FB8FF
MAX_NUCLEONS, MAX_E_PER_SHELL = 12, 4


def f(x) -> str:
    x = round(float(x), 4)
    return str(int(x)) if x == int(x) else repr(x)


def fib_sphere(n, r):
    ga = math.pi * (3 - math.sqrt(5))
    out = []
    for i in range(n):
        y = 1 - (i + 0.5) / n * 2
        rad = math.sqrt(max(0.0, 1 - y * y))
        th = ga * i
        out.append((r * math.cos(th) * rad, r * y, r * math.sin(th) * rad))
    return out


def emit(e):
    sym, z = e["symbol"], e["number"]
    neutrons = int(round(e.get("atomic_mass") or 0)) - z
    shells = [s for s in (e.get("shells") or []) if s > 0]
    nucleons = z + max(0, neutrons)
    lines = [
        f"# std/atoms/{sym}.pwe — {e['name']} atom (Z={z}), schematic (generated).",
        f"# `shape {sym}_atom`: nucleus + electron shells (orbiting). Set on an",
        f"# entity: `entity x {{ shape = {sym}_atom; size = 2.0 }}`.",
        f"module atoms.{sym}",
        "world {",
        f"  shape {sym}_atom {{",
    ]
    # Nucleus.
    if nucleons > 0:
        draw = min(nucleons, MAX_NUCLEONS)
        p_show = max(1, round(draw * z / nucleons)) if z > 0 else 0
        n_show = draw - p_show
        r0 = 0.05 + 0.010 * draw
        pts = fib_sphere(draw, r0)
        for i, (x, y, zz) in enumerate(pts):
            col, rad = (P_COLOR, 0.045) if i < p_show else (N_COLOR, 0.040)
            lines.append(
                f"    part sphere={f(rad)} color=0x{col:06X} at ({f(x)}, {f(y)}, {f(zz)});"
            )
    # Shells: ring + electrons.
    for k, occ in enumerate(shells, start=1):
        radius = 0.30 + 0.16 * (k - 1)
        speed = round(2.6 / k, 3)
        lines.append(f"    part ring=({f(radius)}, 0.010) color=0x{RING_COLOR:06X};")
        e_show = min(occ, MAX_E_PER_SHELL)
        for i in range(e_show):
            phase = round(2 * math.pi * i / e_show + 0.5 * k, 3)
            lines.append(
                f"    part sphere=0.035 color=0x{E_COLOR:06X} "
                f"orbit ({f(radius)}, {f(speed)}, {f(phase)});"
            )
    lines.append("  }")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) != 2:
        print("usage: gen_atoms.py <PeriodicTableJSON.json>", file=sys.stderr)
        return 2
    els = [e for e in json.load(open(sys.argv[1]))["elements"] if 1 <= e["number"] <= 118]
    for e in els:
        (ROOT / "std" / "atoms" / f"{e['symbol']}.pwe").write_text(emit(e))
    print(f"wrote {len(els)} atom modules")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
