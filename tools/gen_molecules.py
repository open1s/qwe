#!/usr/bin/env python3
"""Generate `std/molecules/<name>.pwe`: a molecule as renderable atoms + bonds.

Each module imports the element modules it uses (so `molar_mass()` composes from
`<Sym>.atomic_mass()`), declares one entity per atom (position, CPK colour, size)
with `bond` lines between them (ball-and-stick), and exposes `atom_count()` and
`molar_mass()`. Importing a molecule merges its atoms and bonds into the scene.

    python3 tools/gen_molecules.py

Coordinates are approximate experimental geometries (Ångström).
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# CPK colours (0xRRGGBB) and marker sizes.
COLOR = {
    "H": 0xFFFFFF, "C": 0x909090, "N": 0x3050F8, "O": 0xFF0D0D, "F": 0x90E050,
    "Cl": 0x1FF01F, "Br": 0xA62929, "I": 0x940094, "S": 0xFFFF30, "P": 0xFF8000,
    "Na": 0xAB5CF2, "Mg": 0x8AFF00, "Ca": 0x3DFF00, "Fe": 0xE06633, "K": 0x8F40D4,
}
SIZE = {"H": 0.22, "C": 0.34, "N": 0.30, "O": 0.30, "F": 0.26, "Cl": 0.40,
        "Br": 0.44, "I": 0.48, "S": 0.40, "P": 0.40, "Na": 0.42, "Mg": 0.38}

MOLECULES = {
    # name: atoms [(element, (x,y,z))], bonds [(i, j, order, polarity, cloud)]
    "hydrogen": ([("H", (0.37, 0, 0)), ("H", (-0.37, 0, 0))],
                 [(0, 1, 1, 0.0, False)]),
    "oxygen": ([("O", (0.60, 0, 0)), ("O", (-0.60, 0, 0))],
               [(0, 1, 2, 0.0, True)]),
    "nitrogen": ([("N", (0.55, 0, 0)), ("N", (-0.55, 0, 0))],
                 [(0, 1, 3, 0.0, True)]),
    "water": ([("O", (0, 0, 0)), ("H", (0.757, 0.586, 0)), ("H", (-0.757, 0.586, 0))],
              [(0, 1, 1, 0.35, False), (0, 2, 1, 0.35, False)]),
    "hydrogen_chloride": ([("Cl", (0, 0, 0)), ("H", (1.27, 0, 0))],
                          [(0, 1, 1, 0.55, False)]),
    "carbon_dioxide": ([("C", (0, 0, 0)), ("O", (1.16, 0, 0)), ("O", (-1.16, 0, 0))],
                       [(0, 1, 2, 0.4, True), (0, 2, 2, 0.4, True)]),
    "ammonia": ([("N", (0, 0, -0.28)), ("H", (0.94, 0, 0.24)),
                 ("H", (-0.47, 0.81, 0.24)), ("H", (-0.47, -0.81, 0.24))],
                [(0, 1, 1, 0.3, False), (0, 2, 1, 0.3, False), (0, 3, 1, 0.3, False)]),
    "methane": ([("C", (0, 0, 0)), ("H", (0.629, 0.629, 0.629)),
                 ("H", (-0.629, -0.629, 0.629)), ("H", (-0.629, 0.629, -0.629)),
                 ("H", (0.629, -0.629, -0.629))],
                [(0, 1, 1, 0.15, False), (0, 2, 1, 0.15, False),
                 (0, 3, 1, 0.15, False), (0, 4, 1, 0.15, False)]),
    "sodium_chloride": ([("Na", (0, 0, 0)), ("Cl", (2.36, 0, 0))],
                        [(0, 1, 1, 1.0, False)]),
}


def emit(name, atoms, bonds):
    imports = sorted({a[0] for a in atoms})
    counter = {}
    names = []
    for sym, _ in atoms:
        counter[sym] = counter.get(sym, 0) + 1
        names.append(f"{sym}{counter[sym]}")
    out = [
        f"# std/molecules/{name}.pwe — {name.replace('_', ' ')} (generated).",
        f"# Atoms + `bond` lines (ball-and-stick). Import to render and to use",
        f"# `{name}.molar_mass()` / `{name}.atom_count()`.",
        f"module molecules.{name}",
    ]
    for sym in imports:
        out.append(f'import "../elements/{sym}"')
    out += ["world {", "  gravity = (0, 0, 0)"]
    for (sym, (x, y, z)), nm in zip(atoms, names):
        c = COLOR.get(sym, 0xCCCCCC)
        s = SIZE.get(sym, 0.5)
        out.append(
            f"  entity {nm} {{ position = ({x}, {y}, {z}); shape = sphere; "
            f"size = {s}; color = 0x{c:06X} }}"
        )
    for i, j, order, polarity, cloud in bonds:
        opt = f" order={order}" if order != 1 else ""
        if polarity:
            opt += f" polarity={polarity}"
        if cloud:
            opt += " cloud=true"
        out.append(f"  bond {names[i]} {names[j]}{opt}")
    out += ["}", ""]
    total = " + ".join(f"{nm[:-len(str(counter[sym]))] if False else sym}.atomic_mass()"
                       for sym, _ in atoms)
    out += [
        "funcs {",
        f"  atom_count() {{ {float(len(atoms))} }}",
        f"  molar_mass() {{ {total} }}",
        "}",
    ]
    return "\n".join(out) + "\n"


def main():
    for name, (atoms, bonds) in MOLECULES.items():
        (ROOT / "std" / "molecules" / f"{name}.pwe").write_text(emit(name, atoms, bonds))
    print(f"wrote {len(MOLECULES)} molecules")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
