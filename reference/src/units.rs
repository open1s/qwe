//! Compile-time dimensional analysis for the PWE language.
//!
//! Units are **opt-in and gradual**: a value with no declared unit is
//! *unknown* (a wildcard) and never produces an error, so existing unit-free
//! models are unaffected. Where units are declared — on `state` slots and
//! `params` entries — the checker verifies that every `update`/`rk4` rule is
//! dimensionally consistent, treating the rule as `slot += dt · expr` with
//! `dt` in seconds.
//!
//! Base dimensions: metre, kilogram, second, ampere, kelvin, mole, candela.

// Parse failures carry no context; callers map them to located diagnostics.
#![allow(clippy::result_unit_err)]

/// A dimension vector over the seven SI base units, in the order
/// `[m, kg, s, A, K, mol, cd]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dim(pub [i8; 7]);

/// Base unit names, index-aligned with [`Dim`].
pub const BASE: [&str; 7] = ["m", "kg", "s", "A", "K", "mol", "cd"];

impl Dim {
    pub const ZERO: Dim = Dim([0; 7]);

    pub fn base(i: usize) -> Dim {
        let mut d = [0i8; 7];
        d[i] = 1;
        Dim(d)
    }

    /// The dimension of time (seconds).
    pub fn seconds() -> Dim {
        Dim::base(2)
    }

    pub fn is_dimensionless(&self) -> bool {
        self.0 == [0; 7]
    }

    pub fn times(self, o: Dim) -> Dim {
        let mut d = [0i8; 7];
        for (dst, (a, b)) in d.iter_mut().zip(self.0.iter().zip(o.0.iter())) {
            *dst = a.saturating_add(*b);
        }
        Dim(d)
    }

    pub fn over(self, o: Dim) -> Dim {
        let mut d = [0i8; 7];
        for (dst, (a, b)) in d.iter_mut().zip(self.0.iter().zip(o.0.iter())) {
            *dst = a.saturating_sub(*b);
        }
        Dim(d)
    }

    pub fn pow(self, e: i32) -> Dim {
        let mut d = [0i8; 7];
        for (dst, a) in d.iter_mut().zip(self.0.iter()) {
            *dst = a.saturating_mul(e as i8);
        }
        Dim(d)
    }

    /// Half the exponents (for `sqrt`), or `None` if any exponent is odd.
    pub fn sqrt(self) -> Option<Dim> {
        let mut d = [0i8; 7];
        for (dst, a) in d.iter_mut().zip(self.0.iter()) {
            if a % 2 != 0 {
                return None;
            }
            *dst = a / 2;
        }
        Some(Dim(d))
    }

    /// Human-readable form, e.g. `m/s^2` or `kg*m^2/s^3`.
    pub fn name(&self) -> String {
        if self.is_dimensionless() {
            return "1".to_string();
        }
        // Prefer a named coherent-SI derived unit when the dimensions match.
        for sym in [
            "N", "J", "W", "Pa", "Hz", "C", "V", "F", "Oh", "S", "Wb", "T", "H", "lm", "lx", "Bq",
            "Gy", "Sv", "kat",
        ] {
            if unit_dim(sym) == Some(*self) {
                return sym.to_string();
            }
        }
        let mut num = Vec::new();
        let mut den = Vec::new();
        for (i, &e) in self.0.iter().enumerate() {
            if e == 0 {
                continue;
            }
            let part = if e == 1 {
                BASE[i].to_string()
            } else {
                format!("{}^{}", BASE[i], e)
            };
            if e > 0 {
                num.push(part);
            } else if e == -1 {
                den.push(BASE[i].to_string());
            } else {
                den.push(format!("{}^{}", BASE[i], -e));
            }
        }
        let num = if num.is_empty() {
            "1".to_string()
        } else {
            num.join("*")
        };
        if den.is_empty() {
            num
        } else {
            format!("{num}/{}", den.join("*"))
        }
    }
}

/// The dimension of a unit symbol, base or named coherent-SI derived unit.
fn unit_dim(name: &str) -> Option<Dim> {
    let m = |i: usize| Dim::base(i);
    Some(match name {
        "m" => m(0),
        "kg" => m(1),
        "s" => m(2),
        "A" => m(3),
        "K" => m(4),
        "mol" => m(5),
        "cd" => m(6),
        // Named coherent SI derived units, expanded to base dimensions.
        "Hz" | "Bq" => m(2).pow(-1),
        "N" => m(1).times(m(0)).over(m(2).pow(2)),
        "Pa" => m(1).over(m(0).times(m(2).pow(2))),
        "J" => m(1).times(m(0).pow(2)).over(m(2).pow(2)),
        "W" => m(1).times(m(0).pow(2)).over(m(2).pow(3)),
        "C" => m(3).times(m(2)),
        "V" => m(1).times(m(0).pow(2)).over(m(2).pow(3).times(m(3))),
        "F" => m(3).pow(2).times(m(2).pow(4)).over(m(1).times(m(0).pow(2))),
        "Oh" | "ohm" => m(1).times(m(0).pow(2)).over(m(2).pow(3).times(m(3).pow(2))),
        "S" => m(3).pow(2).times(m(2).pow(3)).over(m(1).times(m(0).pow(2))),
        "Wb" => m(1).times(m(0).pow(2)).over(m(2).pow(2).times(m(3))),
        "T" => m(1).over(m(2).pow(2).times(m(3))),
        "H" => m(1).times(m(0).pow(2)).over(m(2).pow(2).times(m(3).pow(2))),
        "lm" => m(6),
        "lx" => m(6).over(m(0).pow(2)),
        "Gy" | "Sv" => m(0).pow(2).over(m(2).pow(2)),
        "kat" => m(5).over(m(2)),
        _ => return None,
    })
}

impl std::str::FromStr for Dim {
    type Err = ();

    /// Parses a unit expression such as `m`, `m/s^2`, `kg*m/s^2`, `1/s`,
    /// `m^3*kg^-1*s^-2`. Products are left-associative; `^` takes an integer
    /// exponent; a bare number (e.g. the `1` in `1/s`) is dimensionless.
    ///
    /// A parse failure carries no extra context: the only caller turns it into
    /// a language diagnostic keyed by the source offset.
    #[allow(clippy::result_unit_err)]
    fn from_str(text: &str) -> Result<Dim, ()> {
        let mut dim = Dim::ZERO;
        let mut sign = 1i32;
        let mut expect_atom = true;
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'*' => {
                    sign = 1;
                    expect_atom = true;
                    i += 1;
                }
                b'/' => {
                    sign = -1;
                    expect_atom = true;
                    i += 1;
                }
                c if c.is_ascii_digit() => {
                    if !expect_atom {
                        return Err(());
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    expect_atom = false;
                }
                c if c.is_ascii_alphabetic() => {
                    if !expect_atom {
                        return Err(());
                    }
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let atom = unit_dim(&text[start..i]).ok_or(())?;
                    let mut exp = 1i32;
                    if i < bytes.len() && bytes[i] == b'^' {
                        i += 1;
                        let neg = i < bytes.len() && bytes[i] == b'-';
                        if neg {
                            i += 1;
                        }
                        let ds = i;
                        while i < bytes.len() && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        if ds == i {
                            return Err(());
                        }
                        let v: i32 = text[ds..i].parse().map_err(|_| ())?;
                        exp = if neg { -v } else { v };
                    }
                    dim = dim.times(atom.pow(sign * exp));
                    expect_atom = false;
                }
                _ => return Err(()),
            }
        }
        if expect_atom {
            return Err(());
        }
        Ok(dim)
    }
}

/// A value's dimension: `Some(dim)` when annotated, `None` when *unknown*
/// (a wildcard that unifies with anything and never errors).
pub type MaybeDim = Option<Dim>;

/// Unifies two possibly-unknown dimensions. `Err(())` on a definite conflict.
pub fn unify(a: MaybeDim, b: MaybeDim) -> Result<MaybeDim, ()> {
    match (a, b) {
        (Some(x), Some(y)) if x != y => Err(()),
        (Some(x), _) => Ok(Some(x)),
        (_, Some(y)) => Ok(Some(y)),
        (None, None) => Ok(None),
    }
}

/// Product of two possibly-unknown dimensions.
pub fn mul(a: MaybeDim, b: MaybeDim) -> MaybeDim {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.times(y)),
        _ => None,
    }
}

/// Quotient of two possibly-unknown dimensions.
pub fn div(a: MaybeDim, b: MaybeDim) -> MaybeDim {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.over(y)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!("m".parse::<Dim>().unwrap(), Dim::base(0));
        // `kg*m/s^2` is the newton; `name()` prefers the derived symbol.
        assert_eq!("kg*m/s^2".parse::<Dim>().unwrap().name(), "N");
        assert_eq!(
            "m/s^2".parse::<Dim>().unwrap(),
            Dim::base(0).over(Dim::base(2).pow(2))
        );
        assert_eq!("1/s".parse::<Dim>().unwrap(), Dim::base(2).pow(-1));
        assert_eq!(
            "m^3*kg^-1*s^-2".parse::<Dim>().unwrap().name(),
            "m^3/kg*s^2"
        );
        assert!("furlong".parse::<Dim>().is_err());
        assert!("m/".parse::<Dim>().is_err());
    }

    #[test]
    fn unify_conflicts() {
        assert!(unify(Some(Dim::base(0)), Some(Dim::base(2))).is_err());
        assert!(unify(Some(Dim::base(0)), None).is_ok());
        assert!(unify(None, None).is_ok());
    }

    #[test]
    fn derived_si_units_expand_to_bases() {
        let d = |x: &str| x.parse::<Dim>().unwrap();
        assert_eq!(d("N"), d("kg*m/s^2"));
        assert_eq!(d("J"), d("N*m"));
        assert_eq!(d("W"), d("J/s"));
        assert_eq!(d("Pa"), d("N/m^2"));
        assert_eq!(d("Hz"), d("1/s"));
        assert_eq!(d("C"), d("A*s"));
        assert_eq!(d("V"), d("W/A"));
        assert_eq!(d("F"), d("C/V"));
        assert_eq!(d("T"), d("Wb/m^2"));
    }

    #[test]
    fn derived_unit_names_are_printed() {
        let d = |x: &str| x.parse::<Dim>().unwrap();
        assert_eq!(d("kg*m/s^2").name(), "N");
        assert_eq!(d("kg*m^2/s^2").name(), "J");
        assert_eq!(d("1/s").name(), "Hz");
    }
}
