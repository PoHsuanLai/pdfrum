//! The PostScript calculator's operator set (ISO 32000-1 §7.10.5).
//!
//! Forty-two named operators, plus two the parser synthesizes: a procedure
//! (`{ … }`) and a numeric constant. **A token that names none of them
//! becomes a constant**, and a word that will not parse as a number becomes
//! the constant `0.0` — so a program containing `invalid` pushes zero rather
//! than failing.

/// One instruction in a calculator program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PsOp {
    /// `abs`: absolute value.
    Abs,
    /// `add`: sum.
    Add,
    /// `and`: bitwise conjunction of two integers.
    And,
    /// `atan`: arc tangent in degrees, normalized to `[0, 360)`.
    Atan,
    /// `bitshift`: signed shift, left for positive counts.
    Bitshift,
    /// `ceiling`: round towards positive infinity.
    Ceiling,
    /// `copy`: duplicate the top `n` entries.
    Copy,
    /// `cos`: cosine of an angle in **degrees**.
    Cos,
    /// `cvi`: truncate to an integer, saturating.
    Cvi,
    /// `cvr`: convert to a real — a no-op, since everything is a float.
    Cvr,
    /// `div`: quotient, with **division by zero yielding 0**.
    Div,
    /// `dup`: duplicate the top entry.
    Dup,
    /// `eq`: equality, pushing 1.0 or 0.0.
    Eq,
    /// `exch`: swap the top two entries.
    Exch,
    /// `exp`: `base^exponent`.
    Exp,
    /// `false`: push 0.0.
    False,
    /// `floor`: round towards negative infinity.
    Floor,
    /// `ge`: greater or equal.
    Ge,
    /// `gt`: strictly greater.
    Gt,
    /// `idiv`: integer quotient, zero on division by zero or overflow.
    Idiv,
    /// `if`: run the preceding procedure when the condition is true.
    If,
    /// `ifelse`: choose between the two preceding procedures.
    IfElse,
    /// `index`: copy the entry `n` deep.
    Index,
    /// `le`: less or equal.
    Le,
    /// `ln`: natural logarithm, unguarded.
    Ln,
    /// `log`: base-ten logarithm, unguarded.
    Log,
    /// `lt`: strictly less.
    Lt,
    /// `mod`: integer remainder, zero on a zero divisor.
    Mod,
    /// `mul`: product.
    Mul,
    /// `ne`: inequality.
    Ne,
    /// `neg`: negation.
    Neg,
    /// `not`: **logical** negation yielding 0 or 1, not a bitwise complement.
    Not,
    /// `or`: bitwise disjunction of two integers.
    Or,
    /// `pop`: discard the top entry.
    Pop,
    /// `roll`: rotate the top `n` entries by `j`.
    Roll,
    /// `round`: round half **up**, so −5.5 becomes −5.
    Round,
    /// `sin`: sine of an angle in **degrees**.
    Sin,
    /// `sqrt`: square root, NaN for a negative input.
    Sqrt,
    /// `sub`: difference.
    Sub,
    /// `true`: push 1.0.
    True,
    /// `truncate`: a saturating integer round trip, identical to `cvi`.
    Truncate,
    /// `xor`: bitwise exclusive disjunction of two integers.
    Xor,
}

/// The operator table, alphabetically sorted because the C++ binary-searches
/// it. Kept sorted here too so the two stay comparable.
const NAMES: [(&[u8], PsOp); 42] = [
    (b"abs", PsOp::Abs),
    (b"add", PsOp::Add),
    (b"and", PsOp::And),
    (b"atan", PsOp::Atan),
    (b"bitshift", PsOp::Bitshift),
    (b"ceiling", PsOp::Ceiling),
    (b"copy", PsOp::Copy),
    (b"cos", PsOp::Cos),
    (b"cvi", PsOp::Cvi),
    (b"cvr", PsOp::Cvr),
    (b"div", PsOp::Div),
    (b"dup", PsOp::Dup),
    (b"eq", PsOp::Eq),
    (b"exch", PsOp::Exch),
    (b"exp", PsOp::Exp),
    (b"false", PsOp::False),
    (b"floor", PsOp::Floor),
    (b"ge", PsOp::Ge),
    (b"gt", PsOp::Gt),
    (b"idiv", PsOp::Idiv),
    (b"if", PsOp::If),
    (b"ifelse", PsOp::IfElse),
    (b"index", PsOp::Index),
    (b"le", PsOp::Le),
    (b"ln", PsOp::Ln),
    (b"log", PsOp::Log),
    (b"lt", PsOp::Lt),
    (b"mod", PsOp::Mod),
    (b"mul", PsOp::Mul),
    (b"ne", PsOp::Ne),
    (b"neg", PsOp::Neg),
    (b"not", PsOp::Not),
    (b"or", PsOp::Or),
    (b"pop", PsOp::Pop),
    (b"roll", PsOp::Roll),
    (b"round", PsOp::Round),
    (b"sin", PsOp::Sin),
    (b"sqrt", PsOp::Sqrt),
    (b"sub", PsOp::Sub),
    (b"true", PsOp::True),
    (b"truncate", PsOp::Truncate),
    (b"xor", PsOp::Xor),
];

impl PsOp {
    /// The operator a token names, or `None` when it names none — in which
    /// case the token becomes a constant.
    #[must_use]
    pub fn from_name(word: &[u8]) -> Option<Self> {
        NAMES
            .binary_search_by(|(name, _)| (*name).cmp(word))
            .ok()
            .and_then(|i| NAMES.get(i))
            .map(|(_, op)| *op)
    }

    /// Every named operator, for tests and dumps.
    #[allow(
        dead_code,
        reason = "the operator table's completeness is pinned by this module's own tests"
    )]
    pub fn all_named() -> impl Iterator<Item = (&'static [u8], Self)> {
        NAMES.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{NAMES, PsOp};

    #[test]
    fn the_table_is_sorted_so_the_search_works() {
        for pair in NAMES.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            assert!(a.0 < b.0, "{:?} must sort before {:?}", a.0, b.0);
        }
        assert_eq!(NAMES.len(), 42);
    }

    #[test]
    fn every_spelling_resolves_to_its_operator() {
        for (name, op) in PsOp::all_named() {
            assert_eq!(PsOp::from_name(name), Some(op), "for {name:?}");
        }
    }

    #[test]
    fn unknown_tokens_name_no_operator() {
        for word in [&b"invalid"[..], b"", b"Add", b"addx", b"55"] {
            assert!(PsOp::from_name(word).is_none(), "{word:?} should not match");
        }
    }
}
