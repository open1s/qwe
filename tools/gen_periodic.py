#!/usr/bin/env python3
"""Generate PWE periodic-table modules from a JSON dataset.

Source dataset: Bowserinator/Periodic-Table-JSON (MIT), fetched from
https://cdn.jsdelivr.net/gh/Bowserinator/Periodic-Table-JSON@master/PeriodicTableJSON.json
(see std/elements/README.md). Run:

    python3 tools/gen_periodic.py path/to/PeriodicTableJSON.json

It writes `std/periodic.pwe` (lookup functions by atomic number Z) and one
`std/elements/<Symbol>.pwe` module per element (Z = 1..118). Values are numeric;
`0.0` means "not tabulated in the source" (unknown / synthetic / no value).
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

BLOCK = {"s": 1, "p": 2, "d": 3, "f": 4}
# category -> numeric code
CATEGORY = {
    "alkali metal": 1,
    "alkaline earth metal": 2,
    "transition metal": 3,
    "post-transition metal": 4,
    "metalloid": 5,
    "diatomic nonmetal": 6,
    "polyatomic nonmetal": 6,
    "nonmetal": 6,
    "noble gas": 8,
    "lanthanide": 9,
    "actinide": 10,
}
METAL_CATS = {1, 2, 3, 4, 9, 10}


def f(x):
    if x is None:
        return "0.0"
    if isinstance(x, str):
        x = float(x)
    return repr(float(x))


def first(lst):
    return lst[0] if isinstance(lst, list) and lst else None


def load(path):
    data = json.load(open(path))
    out = {}
    for e in data["elements"]:
        z = e.get("number")
        if not z or z > 118:
            continue
        cat = CATEGORY.get(e.get("category"), 0)
        shells = e.get("shells") or []
        phase = (e.get("phase") or "").lower()
        out[z] = {
            "z": z,
            "symbol": e["symbol"],
            "name": e["name"],
            "mass": e.get("atomic_mass"),
            "group": e.get("group") or 0,
            "period": e.get("period") or 0,
            "block": BLOCK.get(e.get("block"), 0),
            "category": cat,
            "en": e.get("electronegativity_pauling"),
            "density": e.get("density"),
            "melt": e.get("melt"),
            "boil": e.get("boil"),
            "ea": e.get("electron_affinity"),
            "ie": first(e.get("ionization_energies")),
            "molar_heat": e.get("molar_heat"),
            "radius": e.get("atomic_radius"),
            "valence": shells[-1] if shells else 0,
            "is_metal": 1 if cat in METAL_CATS else 0,
            "is_metalloid": 1 if cat == 5 else 0,
            "is_nonmetal": 1 if cat == 6 else 0,
            "is_noble": 1 if cat == 8 else 0,
            "is_gas": 1 if phase == "gas" else 0,
        }
    return out


# (pwe function name, data key)
PROPS = [
    ("atomic_mass", "mass"),
    ("group", "group"),
    ("period", "period"),
    ("block", "block"),
    ("category", "category"),
    ("electronegativity", "en"),
    ("density_g_cm3", "density"),
    ("melt_k", "melt"),
    ("boil_k", "boil"),
    ("electron_affinity_kj", "ea"),
    ("ionization_kj", "ie"),
    ("molar_heat_j_mol_k", "molar_heat"),
    ("atomic_radius_pm", "radius"),
    ("valence_electrons", "valence"),
    ("is_metal", "is_metal"),
    ("is_metalloid", "is_metalloid"),
    ("is_nonmetal", "is_nonmetal"),
    ("is_noble_gas", "is_noble"),
    ("is_gas_at_stp", "is_gas"),
]


def nested_if(zs, key):
    # A balanced decision tree over sorted Z (depth ~log2(118)), not a linear
    # chain: keeps parser/lowering recursion shallow and compares less.
    items = sorted((z, f(zs[z][key])) for z in zs)

    def tree(sub):
        if len(sub) == 1:
            z, v = sub[0]
            return f"if(z == {z}, {v}, 0.0)"
        mid = len(sub) // 2
        left, right = sub[:mid], sub[mid:]
        return f"if(z <= {left[-1][0]}, {tree(left)}, {tree(right)})"

    return tree(items)


def periodic_module(els):
    zs = dict(els)
    lines = [
        "# std/periodic.pwe — periodic-table lookup functions (generated; do not edit).",
        "# Source: Bowserinator/Periodic-Table-JSON (MIT). Functions take the atomic",
        "# number `z` (1..118). `0.0` means the property is not tabulated.",
        "module elements.periodic",
        "world { }",
        "",
        "funcs {",
    ]
    for name, key in PROPS:
        lines.append(f"  {name}(z) {{ {nested_if(zs, key)} }}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def element_module(e):
    lines = [
        f"# std/elements/{e['symbol']}.pwe — {e['name']} (Z={e['z']}) (generated).",
        f"module elements.{e['symbol']}",
        "world { }",
        "",
        "funcs {",
        f"  atomic_number() {{ {f(e['z'])} }}",
    ]
    for name, key in PROPS:
        lines.append(f"  {name}() {{ {f(e[key])} }}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) != 2:
        print("usage: gen_periodic.py <PeriodicTableJSON.json>", file=sys.stderr)
        return 2
    els = load(sys.argv[1])
    missing = [z for z in range(1, 119) if z not in els]
    if missing:
        print(f"warning: missing Z={missing}", file=sys.stderr)
    (ROOT / "std" / "periodic.pwe").write_text(periodic_module(els))
    for z, e in els.items():
        (ROOT / "std" / "elements" / f"{e['symbol']}.pwe").write_text(element_module(e))
    print(f"wrote std/periodic.pwe and {len(els)} element modules")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
