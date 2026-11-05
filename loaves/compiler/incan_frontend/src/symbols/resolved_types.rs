//! The checked `ResolvedType` vocabulary, its queries, and its source and descriptor display.

use super::*;

/// Write a nominal type name the way users spell it: a module-qualified union member as `module.Name`.
pub(super) fn write_nominal_name(f: &mut std::fmt::Formatter<'_>, name: &str) -> std::fmt::Result {
    match split_module_qualified_nominal_name(name) {
        Some((module_path, declaration_name)) if !module_path.is_empty() => {
            write!(f, "{}.{declaration_name}", module_path.join("."))
        }
        Some((_, declaration_name)) => write!(f, "{declaration_name}"),
        None => write!(f, "{name}"),
    }
}

/// Write a builtin collection base in the selected presentation style, preserving other nominal names and paths.
pub(super) fn write_type_name(f: &mut std::fmt::Formatter<'_>, name: &str, style: TypeNameStyle) -> std::fmt::Result {
    match collections::from_str(name) {
        Some(id) => f.write_str(style.collection_name(id)),
        None => write_nominal_name(f, name),
    }
}

/// Render canonical Incan source spellings directly from the borrowed semantic shape, without descriptor conversion.
pub(super) fn canonical_incan_type_name(ty: &ResolvedType) -> String {
    DisplayType {
        ty,
        style: TypeNameStyle::Source,
    }
    .to_string()
}

/// The source spelling retained by reflection differs from diagnostic type-descriptor spelling.
#[derive(Clone, Copy)]
pub(super) enum TypeNameStyle {
    Source,
    Descriptor,
}

impl TypeNameStyle {
    /// Keep the source collection policy in the presentation boundary; registry identity remains authoritative.
    fn collection_name(self, id: CollectionTypeId) -> &'static str {
        match self {
            Self::Descriptor => collections::descriptor(id),
            Self::Source => match id {
                CollectionTypeId::List => "list",
                CollectionTypeId::Dict => "dict",
                CollectionTypeId::Set => "set",
                _ => collections::as_str(id),
            },
        }
    }

    /// Use source scalar names while retaining exact numeric widths and existing diagnostic descriptors.
    fn numeric_name(self, id: NumericTypeId) -> &'static str {
        match (self, id) {
            (Self::Source, NumericTypeId::I64) => "int",
            (Self::Source, NumericTypeId::F64) => "float",
            (Self::Source, _) => numerics::as_str(id),
            (Self::Descriptor, _) => numerics::descriptor(id),
        }
    }

    /// Select the registry's source or descriptor spelling without changing string-like type identity.
    fn string_name(self, id: StringLikeId) -> &'static str {
        match self {
            Self::Source => stringlike::as_str(id),
            Self::Descriptor => stringlike::descriptor(id),
        }
    }
}

/// Borrow a resolved type for recursive formatting with one consistent spelling policy.
pub(super) struct DisplayType<'a> {
    ty: &'a ResolvedType,
    style: TypeNameStyle,
}

impl<'a> DisplayType<'a> {
    /// Preserve the selected style when rendering a child type, avoiding intermediate strings or type clones.
    fn nested(&self, ty: &'a ResolvedType) -> Self {
        Self { ty, style: self.style }
    }
}

/// Return whether a resolved field type contains canonical Rust-path metadata at any nesting depth.
pub(crate) fn resolved_type_contains_rust_path(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::RustPath(_) => true,
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => resolved_type_contains_rust_path(inner),
        ResolvedType::FrozenDict(key, value) => {
            resolved_type_contains_rust_path(key) || resolved_type_contains_rust_path(value)
        }
        ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(resolved_type_contains_rust_path),
        ResolvedType::Function(params, result) => {
            params.iter().any(|param| resolved_type_contains_rust_path(&param.ty))
                || resolved_type_contains_rust_path(result)
        }
        _ => false,
    }
}

/// Resolved type (after type checking)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedType {
    /// Internal Rust never type (`!`).
    ///
    /// This is not currently source-spellable in Incan. It preserves diverging Rust callable results so the
    /// typechecker can apply bottom-type compatibility without treating `!` as a nominal Rust path.
    Never,
    /// Primitive types
    Int,
    Float,
    /// Exact-width numeric type introduced by RFC 009.
    Numeric(NumericTypeId),
    Bool,
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    FrozenList(Box<ResolvedType>),
    FrozenDict(Box<ResolvedType>, Box<ResolvedType>),
    FrozenSet(Box<ResolvedType>),
    /// Unit type
    Unit,
    /// Named type (class, model, newtype, enum)
    Named(String),
    /// Canonical checked trait identity retained only as the represented type of a [`Self::TypeToken`].
    ///
    /// A trait has no standalone value or storage representation. Keeping its declaration identity here prevents
    /// aliases, facades and compiled-package imports from being re-resolved by their local spelling after checking.
    Trait(incan_semantics_core::CanonicalSymbolId),
    /// Generic type with arguments
    Generic(String, Vec<ResolvedType>),
    /// Function type, including rest-parameter shape when known.
    Function(Vec<CallableParam>, Box<ResolvedType>),
    /// Value-level token for a source type, e.g. `Type[int]`.
    TypeToken(Box<ResolvedType>),
    /// Tuple type
    Tuple(Vec<ResolvedType>),
    /// Type variable (for generics)
    TypeVar(String),
    /// Self type (resolved to the implementing type in traits)
    SelfType,
    /// Internal reference type (borrowed `&T`).
    ///
    /// ## Notes
    /// - This is currently compiler-internal (not a user-spellable surface type).
    /// - It exists to model Rust interop semantics like `HashMap::get` returning `Option<&V>`.
    Ref(Box<ResolvedType>),
    /// Internal mutable reference type (borrowed `&mut T`).
    ///
    /// ## Notes
    /// - This is currently compiler-internal (not a user-spellable surface type).
    /// - It exists to preserve mutable Rust interop signatures through IR lowering.
    RefMut(Box<ResolvedType>),
    /// Rust import with a known canonical path (`crate::...` string), RFC 041.
    ///
    /// Lowers to backend `IrType::Unknown` until dedicated IR typing exists; provenance also lives on
    /// [`SymbolKind::RustItem`].
    RustPath(String),
    /// Call-site `_` placeholder in bracketed type arguments (RFC 054); resolved away before lowering.
    CallSiteInfer,
    /// Unknown/error type
    Unknown,
}

impl ResolvedType {
    /// Return the semantic type of one numeric registry id.
    ///
    /// RFC 009 makes `int` an alias of `i64` and `float` an alias of `f64`, and an alias creates no separate type
    /// identity, so the `i64` id is [`ResolvedType::Int`] and the `f64` id is [`ResolvedType::Float`] wherever they
    /// come from: any spelling of the type (`int`, `i64`, `long`, `bigint`; `float`, `f64`, `double`, `fp64`), a
    /// literal suffix, or library metadata. Every other numeric id keeps its exact-width type; the registry's `bool`
    /// entry is [`ResolvedType::Bool`].
    pub fn from_numeric_id(id: NumericTypeId) -> ResolvedType {
        match id {
            NumericTypeId::I64 => ResolvedType::Int,
            NumericTypeId::F64 => ResolvedType::Float,
            NumericTypeId::Bool => ResolvedType::Bool,
            _ => ResolvedType::Numeric(id),
        }
    }

    /// Return the semantic type of one string-like registry id; an f-string is a `str` at the type level.
    pub fn from_string_like_id(id: StringLikeId) -> ResolvedType {
        match id {
            StringLikeId::Str | StringLikeId::FString => ResolvedType::Str,
            StringLikeId::Bytes => ResolvedType::Bytes,
            StringLikeId::FrozenStr => ResolvedType::FrozenStr,
            StringLikeId::FrozenBytes => ResolvedType::FrozenBytes,
        }
    }

    /// Return the builtin type a simple name spells, or `None` for any other name.
    ///
    /// The spellings are `incan_lang`'s (one rule for every registry, so `str`, `Str` and `String` are one type); this
    /// is the only place that maps them to semantic types, and every resolver calls it. A bare collection base stays a
    /// named type in its canonical spelling (`Tuple` as "any tuple").
    pub fn builtin_simple(name: &str) -> Option<ResolvedType> {
        if name == conventions::UNIT_TYPE_NAME || name == conventions::NONE_TYPE_NAME {
            return Some(ResolvedType::Unit);
        }
        Some(match builtin_type_name(name)? {
            BuiltinTypeName::Numeric(id) => ResolvedType::from_numeric_id(id),
            BuiltinTypeName::StringLike(id) => ResolvedType::from_string_like_id(id),
            BuiltinTypeName::Collection(id) => ResolvedType::Named(collections::as_str(id).to_string()),
            BuiltinTypeName::Ownership(mode) => ResolvedType::Named(mode.as_str().to_string()),
        })
    }

    /// Return the builtin type a collection base applied to `args` spells, with the base in its canonical spelling.
    pub fn builtin_generic(id: CollectionTypeId, args: Vec<ResolvedType>) -> ResolvedType {
        let argument = |index: usize| Box::new(args.get(index).cloned().unwrap_or(ResolvedType::Unknown));
        match id {
            CollectionTypeId::FrozenList => ResolvedType::FrozenList(argument(0)),
            CollectionTypeId::FrozenSet => ResolvedType::FrozenSet(argument(0)),
            CollectionTypeId::FrozenDict => ResolvedType::FrozenDict(argument(0), argument(1)),
            _ => ResolvedType::Generic(collections::as_str(id).to_string(), args),
        }
    }

    /// Check if this is a Result type
    pub fn is_result(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result)
        )
    }

    /// Check if this is an Option type
    pub fn is_option(&self) -> bool {
        matches!(
            self,
            ResolvedType::Generic(name, _) if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option)
        )
    }

    /// The element types of a tuple type, spelled `tuple[...]` or as a tuple literal's own type: both checked
    /// encodings denote the same structural slots.
    pub fn tuple_elements(&self) -> Option<&[ResolvedType]> {
        match self {
            ResolvedType::Tuple(elements) => Some(elements),
            ResolvedType::Generic(name, elements)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                Some(elements)
            }
            _ => None,
        }
    }

    /// Check if this is an anonymous union type.
    pub fn is_union(&self) -> bool {
        matches!(self, ResolvedType::Generic(name, _) if name == UNION_TYPE_NAME)
    }

    /// Get the normalized member list from `Union[...]`.
    pub fn union_members(&self) -> Option<&[ResolvedType]> {
        match self {
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => Some(args.as_slice()),
            _ => None,
        }
    }

    /// Get the Ok type from Result[T, E]
    pub fn result_ok_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Get the Err type from `Result[T, E]`.
    pub fn result_err_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Result) && args.len() >= 2 =>
            {
                Some(&args[1])
            }
            _ => None,
        }
    }

    /// Get the inner type from `Option[T]`.
    pub fn option_inner_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Option) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Get the yielded element type from `Generator[T]`.
    pub fn generator_element_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if collections::from_str(name.as_str()) == Some(CollectionTypeId::Generator) && !args.is_empty() =>
            {
                Some(&args[0])
            }
            _ => None,
        }
    }

    /// Return the canonical owned Incan `Iterator[T]` item type.
    pub fn iterator_item_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Generic(name, args)
                if traits::from_qualified_str(name) == Some(TraitId::Iterator) && args.len() == 1 =>
            {
                args.first()
            }
            _ => None,
        }
    }

    /// Return the item type accepted by the direct `zip(left, right)` builtin.
    ///
    /// The backend can currently adapt lists, frozen lists, and canonical source-owned iterators without broadening
    /// this builtin to every value accepted by ordinary `for` iteration. Unknown types remain accepted for diagnostic
    /// recovery; all other unsupported operands are rejected by the builtin call checker.
    pub fn builtin_zip_item_type(&self) -> Option<&ResolvedType> {
        match self {
            ResolvedType::Unknown => Some(self),
            ResolvedType::FrozenList(inner) => Some(inner),
            ResolvedType::Generic(name, args)
                if (name == surface_types::as_str(SurfaceTypeId::Vec)
                    || matches!(
                        collections::from_str(name),
                        Some(CollectionTypeId::List | CollectionTypeId::FrozenList)
                    ))
                    && args.len() == 1 =>
            {
                args.first()
            }
            _ => self.iterator_item_type(),
        }
    }
}

impl std::fmt::Display for ResolvedType {
    /// Format a resolved type using diagnostic type descriptors; reflection selects source spelling separately.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(
            &DisplayType {
                ty: self,
                style: TypeNameStyle::Descriptor,
            },
            f,
        )
    }
}

impl std::fmt::Display for DisplayType<'_> {
    /// Format the borrowed semantic tree recursively, keeping shape and parameter markers independent of spelling.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.ty {
            ResolvedType::Never => write!(f, "!"),
            ResolvedType::Int => f.write_str(self.style.numeric_name(NumericTypeId::I64)),
            ResolvedType::Float => f.write_str(self.style.numeric_name(NumericTypeId::F64)),
            ResolvedType::Numeric(id) => f.write_str(self.style.numeric_name(*id)),
            ResolvedType::Bool => f.write_str(self.style.numeric_name(NumericTypeId::Bool)),
            ResolvedType::Str => f.write_str(self.style.string_name(StringLikeId::Str)),
            ResolvedType::Bytes => f.write_str(self.style.string_name(StringLikeId::Bytes)),
            ResolvedType::FrozenStr => f.write_str(self.style.string_name(StringLikeId::FrozenStr)),
            ResolvedType::FrozenBytes => f.write_str(self.style.string_name(StringLikeId::FrozenBytes)),
            ResolvedType::FrozenList(elem) => {
                write!(
                    f,
                    "{}[{}]",
                    self.style.collection_name(CollectionTypeId::FrozenList),
                    self.nested(elem)
                )
            }
            ResolvedType::FrozenDict(k, v) => {
                write!(
                    f,
                    "{}[{}, {}]",
                    self.style.collection_name(CollectionTypeId::FrozenDict),
                    self.nested(k),
                    self.nested(v)
                )
            }
            ResolvedType::FrozenSet(elem) => {
                write!(
                    f,
                    "{}[{}]",
                    self.style.collection_name(CollectionTypeId::FrozenSet),
                    self.nested(elem)
                )
            }
            ResolvedType::Unit => write!(f, "Unit"),
            ResolvedType::Named(name) => write_type_name(f, name, self.style),
            ResolvedType::Trait(identity) => write_type_name(f, &identity.declaration_name, self.style),
            ResolvedType::Generic(name, args) => {
                write_type_name(f, name, self.style)?;
                write!(f, "[")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", self.nested(arg))?;
                }
                write!(f, "]")
            }
            ResolvedType::Function(params, ret) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match p.kind {
                        ParamKind::Normal if p.is_mut => write!(f, "mut {}", self.nested(&p.ty))?,
                        ParamKind::Normal => write!(f, "{}", self.nested(&p.ty))?,
                        ParamKind::RestPositional => write!(f, "*{}", self.nested(&p.ty))?,
                        ParamKind::RestKeyword => write!(f, "**{}", self.nested(&p.ty))?,
                    }
                }
                write!(f, ") -> {}", self.nested(ret))
            }
            ResolvedType::TypeToken(inner) => write!(f, "Type[{}]", self.nested(inner)),
            ResolvedType::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", self.nested(e))?;
                }
                write!(f, ")")
            }
            ResolvedType::TypeVar(name) => write!(f, "{}", name),
            ResolvedType::SelfType => write!(f, "Self"),
            ResolvedType::Ref(inner) => write!(f, "&{}", self.nested(inner)),
            ResolvedType::RefMut(inner) => write!(f, "&mut {}", self.nested(inner)),
            ResolvedType::RustPath(path) => write!(f, "rust::{}", path),
            ResolvedType::CallSiteInfer => write!(f, "_"),
            ResolvedType::Unknown => write!(f, "?"),
        }
    }
}

/// Construct the canonical semantic form for an anonymous union.
///
/// This flattens nested unions, removes duplicates, sorts members by display for deterministic equality, and rewrites
/// `None`/`Unit`-containing unions through `Option[...]` as required by RFC 029.
pub fn union_ty(members: Vec<ResolvedType>) -> ResolvedType {
    let mut flattened = Vec::new();
    let mut contains_none = false;

    for member in members {
        match member {
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => flattened.extend(args),
            ResolvedType::Unit => contains_none = true,
            other => flattened.push(other),
        }
    }

    // Two members can display alike (a module-qualified member displays as the path users write), so equal displays
    // fall back to the full representation to keep the order deterministic.
    flattened.sort_by(|left, right| {
        left.to_string()
            .cmp(&right.to_string())
            .then_with(|| format!("{left:?}").cmp(&format!("{right:?}")))
    });
    flattened.dedup();

    let inner = match flattened.as_slice() {
        [] => ResolvedType::Unit,
        [single] => single.clone(),
        _ => ResolvedType::Generic(UNION_TYPE_NAME.to_string(), flattened),
    };

    if contains_none {
        ResolvedType::Generic(collections::as_str(CollectionTypeId::Option).to_string(), vec![inner])
    } else {
        inner
    }
}
