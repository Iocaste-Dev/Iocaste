//! Semantic types used by the checker.

use std::collections::HashMap;

pub type TypeId = usize;
pub type ParamId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    Bool,
    Byte,
    SByte,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    USize,
    ISize,
    Float,
    Double,
    Char,
    String,
}

impl Prim {
    pub const ALL: [Prim; 15] = [
        Prim::Bool, Prim::Byte, Prim::SByte, Prim::Short, Prim::UShort, Prim::Int, Prim::UInt,
        Prim::Long, Prim::ULong, Prim::USize, Prim::ISize, Prim::Float, Prim::Double, Prim::Char,
        Prim::String,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Prim::Bool => "bool",
            Prim::Byte => "byte",
            Prim::SByte => "sbyte",
            Prim::Short => "short",
            Prim::UShort => "ushort",
            Prim::Int => "int",
            Prim::UInt => "uint",
            Prim::Long => "long",
            Prim::ULong => "ulong",
            Prim::USize => "usize",
            Prim::ISize => "isize",
            Prim::Float => "float",
            Prim::Double => "double",
            Prim::Char => "char",
            Prim::String => "string",
        }
    }

    pub fn from_name(s: &str) -> Option<Prim> {
        Prim::ALL.iter().copied().find(|p| p.name() == s)
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Prim::Byte | Prim::SByte | Prim::Short | Prim::UShort | Prim::Int | Prim::UInt | Prim::Long
                | Prim::ULong | Prim::USize | Prim::ISize
        )
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::Float | Prim::Double)
    }

    pub fn is_numeric(self) -> bool {
        self.is_integer() || self.is_float()
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Prim::SByte | Prim::Short | Prim::Int | Prim::Long | Prim::ISize | Prim::Float | Prim::Double)
    }

    /// Implicit numeric widening (never loses the sign or the magnitude class).
    pub fn widens_to(self, to: Prim) -> bool {
        use Prim::*;
        if self == to {
            return true;
        }
        match self {
            Byte => matches!(to, Short | UShort | Int | UInt | Long | ULong | USize | ISize | Float | Double),
            SByte => matches!(to, Short | Int | Long | ISize | Float | Double),
            Short => matches!(to, Int | Long | ISize | Float | Double),
            UShort => matches!(to, Int | UInt | Long | ULong | USize | ISize | Float | Double),
            Int => matches!(to, Long | ISize | Float | Double),
            UInt => matches!(to, Long | ULong | USize | Float | Double),
            Long => matches!(to, Float | Double),
            ULong => matches!(to, Float | Double),
            USize => matches!(to, ULong | Float | Double),
            ISize => matches!(to, Long | Float | Double),
            Float => matches!(to, Double),
            Char => matches!(to, UShort | Int | UInt | Long | ULong | USize | Float | Double),
            _ => false,
        }
    }

    /// Result type of a binary arithmetic operation on two numeric types.
    pub fn promote(a: Prim, b: Prim) -> Option<Prim> {
        use Prim::*;
        if !a.is_numeric() || !b.is_numeric() {
            return None;
        }
        if a == Double || b == Double {
            return Some(Double);
        }
        if a == Float || b == Float {
            return Some(Float);
        }
        let norm = |p: Prim| match p {
            Byte | SByte | Short | UShort => Int,
            x => x,
        };
        let (a, b) = (norm(a), norm(b));
        if a == b {
            Some(a)
        } else if a.widens_to(b) {
            Some(b)
        } else if b.widens_to(a) {
            Some(a)
        } else {
            Some(Long)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    /// An error was already reported; suppresses follow-up errors.
    Error,
    /// The type of `return`, `break`, ... : fits anywhere.
    Never,
    Void,
    Prim(Prim),
    Named { id: TypeId, args: Vec<Ty> },
    Array(Box<Ty>),
    Ref { mutable: bool, inner: Box<Ty> },
    /// A generic type parameter (`T`) inside its own declaration.
    Param(ParamId),
    /// An inference variable.
    Infer(u32),
}

impl Ty {
    pub fn contains(&self, pred: &dyn Fn(&Ty) -> bool) -> bool {
        if pred(self) {
            return true;
        }
        match self {
            Ty::Named { args, .. } => args.iter().any(|a| a.contains(pred)),
            Ty::Array(t) => t.contains(pred),
            Ty::Ref { inner, .. } => inner.contains(pred),
            _ => false,
        }
    }

    pub fn has_error(&self) -> bool {
        self.contains(&|t| matches!(t, Ty::Error))
    }

    pub fn has_infer(&self) -> bool {
        self.contains(&|t| matches!(t, Ty::Infer(_)))
    }
}

/// Replace generic parameters according to `map`.
pub fn subst(ty: &Ty, map: &HashMap<ParamId, Ty>) -> Ty {
    if map.is_empty() {
        return ty.clone();
    }
    match ty {
        Ty::Param(p) => map.get(p).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Named { id, args } => Ty::Named { id: *id, args: args.iter().map(|a| subst(a, map)).collect() },
        Ty::Array(t) => Ty::Array(Box::new(subst(t, map))),
        Ty::Ref { mutable, inner } => Ty::Ref { mutable: *mutable, inner: Box::new(subst(inner, map)) },
        _ => ty.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening() {
        assert!(Prim::Int.widens_to(Prim::Long));
        assert!(Prim::Int.widens_to(Prim::Double));
        assert!(!Prim::Double.widens_to(Prim::Float));
        assert!(!Prim::Int.widens_to(Prim::UInt));
        assert!(Prim::Byte.widens_to(Prim::UInt));
    }

    #[test]
    fn promotion() {
        assert_eq!(Prim::promote(Prim::Int, Prim::Double), Some(Prim::Double));
        assert_eq!(Prim::promote(Prim::Short, Prim::Byte), Some(Prim::Int));
        assert_eq!(Prim::promote(Prim::Int, Prim::UInt), Some(Prim::Long));
        assert_eq!(Prim::promote(Prim::Int, Prim::Bool), None);
    }
}
