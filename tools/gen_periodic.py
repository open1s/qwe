#!/usr/bin/env python3
"""Generate PWE periodic-table modules from a JSON dataset.

Source: Bowserinator/Periodic-Table-JSON (MIT), via
https://cdn.jsdelivr.net/gh/Bowserinator/Periodic-Table-JSON@master/PeriodicTableJSON.json
(see std/elements/README.md). Run:

    python3 tools/gen_periodic.py path/to/PeriodicTableJSON.json

Writes `std/periodic.pwe` (lookup by atomic number Z) and `std/elements/<Sym>.pwe`
(Z = 1..118). Numeric only; `0.0` = not tabulated. Lookups are emitted as
**balanced decision trees** (depth ~log2(n)) to keep parser/lowering recursion
shallow.
"""
import json
import sys
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]
BLOCK = {"s": 1, "p": 2, "d": 3, "f": 4}
CATEGORY = {
    "alkali metal": 1, "alkaline earth metal": 2, "transition metal": 3,
    "post-transition metal": 4, "metalloid": 5, "diatomic nonmetal": 6,
    "polyatomic nonmetal": 6, "nonmetal": 6, "noble gas": 8,
    "lanthanide": 9, "actinide": 10,
}
METAL_CATS = {1, 2, 3, 4, 9, 10}


def fnum(x) -> str:
    if x is None:
        return "0.0"
    if isinstance(x, str):
        x = float(x)
    return repr(float(x))


def balanced(items, var: str) -> str:
    """A balanced decision tree over `items = [(key, expr)]` on `var`."""
    items = sorted(items)
    if len(items) == 1:
        k, e = items[0]
        return f"if({var} == {k}, {e}, 0.0)"
    mid = len(items) // 2
    left, right = items[:mid], items[mid:]
    return f"if({var} <= {left[-1][0]}, {balanced(left, var)}, {balanced(right, var)})"


def first(lst):
    return lst[0] if isinstance(lst, list) and lst else None


def load(path):
    out = {}
    for e in json.load(open(path))["elements"]:
        z = e.get("number")
        if not z or z > 118:
            continue
        cat = CATEGORY.get(e.get("category"), 0)
        shells = e.get("shells") or []
        phase = (e.get("phase") or "").lower()
        mass = e.get("atomic_mass")
        neutrons = int(round(mass)) - z if mass else 0
        out[z] = {
            "z": z, "symbol": e["symbol"], "name": e["name"], "mass": mass,
            "group": e.get("group") or 0, "period": e.get("period") or 0,
            "block": BLOCK.get(e.get("block"), 0), "category": cat,
            "en": e.get("electronegativity_pauling"), "density": e.get("density"),
            "melt": e.get("melt"), "boil": e.get("boil"),
            "ea": e.get("electron_affinity"), "ie": first(e.get("ionization_energies")),
            "molar_heat": e.get("molar_heat"), "radius": e.get("atomic_radius"),
            "valence": shells[-1] if shells else 0,
            "electrons": z, "neutrons": neutrons, "shells": shells,
            "shell_count": len(shells),
            "is_metal": 1 if cat in METAL_CATS else 0,
            "is_metalloid": 1 if cat == 5 else 0,
            "is_nonmetal": 1 if cat == 6 else 0,
            "is_noble": 1 if cat == 8 else 0,
            "is_gas": 1 if phase == "gas" else 0,
        }
    return out


PROPS = [
    ("atomic_mass", "mass"), ("group", "group"), ("period", "period"),
    ("block", "block"), ("category", "category"), ("electronegativity", "en"),
    ("density_g_cm3", "density"), ("melt_k", "melt"), ("boil_k", "boil"),
    ("electron_affinity_kj", "ea"), ("ionization_kj", "ie"),
    ("molar_heat_j_mol_k", "molar_heat"), ("atomic_radius_pm", "radius"),
    ("valence_electrons", "valence"), ("electrons", "electrons"),
    ("neutrons", "neutrons"), ("shell_count", "shell_count"),
    ("is_metal", "is_metal"), ("is_metalloid", "is_metalloid"),
    ("is_nonmetal", "is_nonmetal"), ("is_noble_gas", "is_noble"),
    ("is_gas_at_stp", "is_gas"),
]
MAX_SHELLS = 7


def periodic_module(els):
    zs = dict(els)
    lines = [
        "# std/periodic.pwe — periodic-table lookup functions (generated; do not edit).",
        "# Source: Bowserinator/Periodic-Table-JSON (MIT). `z` = atomic number",
        "# (1..118); `k` = shell index (1..7). `0.0` = not tabulated.",
        "module elements.periodic",
        "world { }",
        "",
        "funcs {",
    ]
    for name, key in PROPS:
        lines.append(f"  {name}(z) {{ {balanced([(z, fnum(zs[z][key])) for z in zs], 'z')} }}")
    # shell_electrons(z, k): outer tree over k, inner tree over z.
    outer = []
    for k in range(1, MAX_SHELLS + 1):
        inner = balanced([(z, fnum(zs[z]["shells"][k - 1] if k - 1 < len(zs[z]["shells"]) else 0)) for z in zs], "z")
        outer.append((k, inner))
    lines.append(f"  shell_electrons(z, k) {{ {balanced(outer, 'k')} }}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def element_module(e):
    shells = e["shells"]
    lines = [
        f"# std/elements/{e['symbol']}.pwe — {e['name']} (Z={e['z']}) (generated).",
        f"module elements.{e['symbol']}", "world { }", "", "funcs {",
        f"  atomic_number() {{ {fnum(e['z'])} }}",
    ]
    for name, key in PROPS:
        lines.append(f"  {name}() {{ {fnum(e[key])} }}")
    lines.append(
        "  shell_electrons(k) { "
        + balanced([(i + 1, fnum(n)) for i, n in enumerate(shells)], "k")
        + " }"
    )
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) != 2:
        print("usage: gen_periodic.py <PeriodicTableJSON.json>", file=sys.stderr)
        return 2
    els = load(sys.argv[1])
    (ROOT / "std" / "periodic.pwe").write_text(periodic_module(els))
    for e in els.values():
        (ROOT / "std" / "elements" / f"{e['symbol']}.pwe").write_text(element_module(e))
    print(f"wrote std/periodic.pwe and {len(els)} element modules")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
