# `std/elements/` — the periodic table

Generated PWE modules: one `<Symbol>.pwe` per element (Z = 1..118) plus
`std/periodic.pwe` with lookup functions by atomic number.

## Data source

Derived from **Bowserinator/Periodic-Table-JSON** (MIT License), fetched from
`https://cdn.jsdelivr.net/gh/Bowserinator/Periodic-Table-JSON@master/PeriodicTableJSON.json`.
Regenerate with:

```sh
python3 tools/gen_periodic.py path/to/PeriodicTableJSON.json
```

Do not edit these files by hand (the header says "generated"); edit the
generator instead.

## Convention

Values are numeric; **`0.0` means "not tabulated in the source"** (unknown,
synthetic, or no defined value — e.g. electronegativity of the noble gases,
or properties of the shortest-lived elements). Units:

| Accessor | Unit / meaning |
| --- | --- |
| `atomic_mass` | u (standard atomic weight) |
| `group`, `period` | periodic-table position (`0` if n/a, e.g. lanthanides) |
| `block` | `1`=s, `2`=p, `3`=d, `4`=f |
| `category` | `1` alkali, `2` alkaline, `3` transition, `4` post-transition, `5` metalloid, `6` nonmetal, `8` noble gas, `9` lanthanide, `10` actinide |
| `electronegativity` | Pauling |
| `density_g_cm3` | g/cm³ |
| `melt_k`, `boil_k` | kelvin |
| `electron_affinity_kj` | kJ/mol |
| `ionization_kj` | first ionization energy, kJ/mol |
| `molar_heat_j_mol_k` | J/(mol·K) |
| `atomic_radius_pm` | pm (empirical) |
| `valence_electrons` | electrons in the outermost occupied shell |
| `is_metal`, `is_metalloid`, `is_nonmetal`, `is_noble_gas`, `is_gas_at_stp` | `1.0` / `0.0` |

## Usage

```pwe
import "std/periodic"          # lookup by Z
import "std/elements/Fe"       # or import one element

world { gravity=(0,0,0) entity e { state=(m=0.0, en=0.0) } }
systems { update { on=e; dt=1.0
  m  = periodic.atomic_mass(26.0) + 0.0   # 55.845
  en = Fe.electronegativity() + 0.0       # 1.83
} }
```
