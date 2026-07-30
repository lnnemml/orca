//! Element symbol ↔ atomic number, shared by every artifact reader.
//!
//! Used to cross-check a `$Geometry` block's element column against a population
//! block's `&ATNO` array (atomic numbers): the two must agree in order, or the
//! charges would be shown on the wrong atoms (the seam ADR-010/ADR-012 guard).

/// Symbols indexed by atomic number. `SYMBOLS[0]` is a placeholder so `SYMBOLS[z]`
/// reads naturally for `z` in 1..=118.
const SYMBOLS: [&str; 119] = [
    "n", // 0 — placeholder (neutron / none), never a real atom
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si",
    "P", "S", "Cl", "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni",
    "Cu", "Zn", "Ga", "Ge", "As", "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb",
    "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In", "Sn", "Sb", "Te", "I", "Xe",
    "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb", "Dy", "Ho",
    "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg",
    "Tl", "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np",
    "Pu", "Am", "Cm", "Bk", "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg",
    "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh", "Fl", "Mc", "Lv", "Ts", "Og",
];

/// Atomic number for an element symbol (case-sensitive, ORCA's own casing). A
/// fragment suffix like `C(1)` (measured in GOAT/xTB `$Geometry`) must be stripped
/// by the caller before calling this.
pub fn z_of(symbol: &str) -> Option<u8> {
    SYMBOLS
        .iter()
        .position(|&s| s == symbol)
        .filter(|&z| z > 0)
        .map(|z| z as u8)
}

/// Element symbol for an atomic number (1..=118).
pub fn symbol_of(z: u8) -> Option<&'static str> {
    let z = z as usize;
    if (1..SYMBOLS.len()).contains(&z) {
        Some(SYMBOLS[z])
    } else {
        None
    }
}

/// Strip an ORCA fragment suffix, e.g. `C(1)` → `C` (measured in GOAT/xTB
/// `$Geometry`; ordinary jobs have a bare symbol).
pub fn strip_fragment_suffix(symbol: &str) -> &str {
    match symbol.find('(') {
        Some(i) => &symbol[..i],
        None => symbol,
    }
}
