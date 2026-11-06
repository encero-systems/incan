//! Rust type ASTs for planned types: carriers, references, collections, generators and callable objects.

use super::*;

/// Retain the standard library's absolute carrier identity independently of source nominal spellings.
pub(super) fn standard_path(segments: &[&str], span: Span) -> ast::Path {
    let mut path = ast::Path {
        span,
        segments: thin_vec![
            ast::PathSegment::path_root(span),
            ast::PathSegment::from_ident(ident("std", span))
        ],
        tokens: None,
    };
    for segment in segments {
        path.segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    path
}

/// The standard enum a native Option/Result carrier aliases, below `std`, and its type arguments in variant order.
pub(crate) fn standard_carrier(carrier: &crate::plan::EnumDeclaration) -> ([&'static str; 2], Vec<PlanType>) {
    let path = if carrier.carrier == Carrier::Option {
        ["option", "Option"]
    } else {
        ["result", "Result"]
    };
    let arguments = carrier
        .variants
        .iter()
        .flat_map(|variant| variant.fields.iter().cloned())
        .collect();
    (path, arguments)
}

/// Spell a carried view's result as its standard enum, so each payload reference is a signature loan position.
pub(super) fn carried_output_ty(carrier: &crate::plan::EnumDeclaration, span: Span) -> Box<ast::Ty> {
    let (segments, arguments) = standard_carrier(carrier);
    let mut path = standard_path(&segments, span);
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: arguments
                .iter()
                .map(|argument| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(argument, span))))
                .collect(),
        })));
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Name the shared builtin carrier directly, so compiling the runtime never needs its own already-built facade.
pub(super) fn runtime_carrier_ty(carrier: RuntimeCarrier, span: Span) -> ast::TyKind {
    let mut segments = thin_vec![ast::PathSegment::path_root(span)];
    for segment in carrier.path().split("::") {
        segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    ast::TyKind::Path(
        None,
        ast::Path {
            span,
            segments,
            tokens: None,
        },
    )
}

/// Spell `::std::boxed::Box` from the crate root, so a source declaration named `Box` cannot shadow a closure's box.
pub(crate) fn boxed_path(span: Span) -> ast::Path {
    let mut segments = thin_vec![ast::PathSegment::path_root(span)];
    for name in ["std", "boxed", "Box"] {
        segments.push(ast::PathSegment::from_ident(ident(name, span)));
    }
    ast::Path {
        span,
        segments,
        tokens: None,
    }
}

/// Declare the exact owned or borrowed runtime handle, leaving other plan types to their ordinary declaration path.
pub(super) fn generator_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let (name, leaf, depth, mutability) = match kind {
        PlanType::Generator(leaf, depth) => ("Generator", leaf, *depth, None),
        PlanType::GeneratorMutRef(leaf, depth) => ("Generator", leaf, *depth, Some(ast::Mutability::Mut)),
        PlanType::GeneratorYield(leaf, depth) => ("GeneratorYield", leaf, *depth, None),
        PlanType::GeneratorYieldRef(leaf, depth) => ("GeneratorYield", leaf, *depth, Some(ast::Mutability::Not)),
        _ => return None,
    };
    let owned = generator_ty(name, leaf, depth, span);
    Some(match mutability {
        None => owned,
        Some(mutbl) => Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Ref(None, ast::MutTy { ty: owned, mutbl }),
            span,
            tokens: None,
        }),
    })
}

/// Build an exact shared or mutable scalar reference after plan preflight validates its single referent; malformed
/// arity stays an inferred error type.
pub(super) fn scalar_reference_ast(elements: &[PlanType], mutable: bool, span: Span) -> ast::TyKind {
    let [inner] = elements else {
        return ast::TyKind::Infer;
    };
    ast::TyKind::Ref(
        None,
        ast::MutTy {
            ty: ty(inner, span),
            mutbl: if mutable {
                ast::Mutability::Mut
            } else {
                ast::Mutability::Not
            },
        },
    )
}

/// Construct an admitted native AST type without generating or parsing Rust source.
pub(crate) fn ty(kind: &PlanType, span: Span) -> Box<ast::Ty> {
    if let Some(ty) = static_reference_ty(kind, span) {
        return ty;
    }
    if let PlanType::CChar = kind {
        return Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Path(None, standard_path(&["ffi", "c_char"], span)),
            span,
            tokens: None,
        });
    }
    if let PlanType::RawPointer(mutable, elements) = kind {
        let pointee = elements
            .first()
            .map(|pointee| ty(pointee, span))
            .unwrap_or_else(|| ty(&PlanType::Unit, span));
        return Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Ptr(ast::MutTy {
                ty: pointee,
                mutbl: if *mutable {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            }),
            span,
            tokens: None,
        });
    }
    if let PlanType::TypeToken(elements) = kind {
        return type_token_ty(elements, span);
    }
    if let PlanType::FrozenCollectionRef(name, elements) = kind {
        return Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Ref(
                None,
                ast::MutTy {
                    ty: frozen_collection_ty(name, elements, span),
                    mutbl: ast::Mutability::Not,
                },
            ),
            span,
            tokens: None,
        });
    }
    if let PlanType::FrozenCollection(name, elements) = kind {
        return frozen_collection_ty(name, elements, span);
    }
    if let Some(ty) = zip_signature_ty(kind, span) {
        return ty;
    }
    if let Some(ty) = generator_signature_ty(kind, span) {
        return ty;
    }
    if let Some(ty) = collection_signature_ty(kind, span) {
        return ty;
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: admitted_value_ty_kind(kind, span),
        span,
        tokens: None,
    })
}

/// Construct the value type after static, pointer, and specialized carrier signatures have been admitted.
pub(super) fn admitted_value_ty_kind(kind: &PlanType, span: Span) -> ast::TyKind {
    match kind {
        PlanType::ScalarRef(elements) => scalar_reference_ast(elements, false, span),
        PlanType::ScalarMutRef(elements) => scalar_reference_ast(elements, true, span),
        PlanType::Future(arguments) => {
            let name = arguments.first().map(crate::futures::alias_name).unwrap_or_default();
            ast::TyKind::Path(None, ast::Path::from_ident(ident(&name, span)))
        }
        PlanType::String => ast::TyKind::Path(None, standard_path(&["string", "String"], span)),
        PlanType::NativeStr => ast::TyKind::Path(None, ast::Path::from_ident(ident("str", span))),
        PlanType::StrRef => reference_ast(ty(&PlanType::NativeStr, span), false),
        PlanType::NativeSlice(elements) => elements
            .first()
            .map_or(ast::TyKind::Infer, |element| ast::TyKind::Slice(ty(element, span))),
        PlanType::SliceRef(elements) | PlanType::SliceMutRef(elements) => reference_ast(
            ty(&PlanType::NativeSlice(elements.clone()), span),
            matches!(kind, PlanType::SliceMutRef(_)),
        ),
        PlanType::CallableArguments(inputs) => ast::TyKind::Tup(inputs.iter().map(|input| ty(input, span)).collect()),
        PlanType::Tuple(elements) => ast::TyKind::Tup(
            elements
                .iter()
                .map(|element| ty(&tuple_element_type(element.clone()), span))
                .collect(),
        ),
        PlanType::TupleRef(elements) => reference_ast(ty(&PlanType::Tuple(elements.clone()), span), false),
        PlanType::TupleMutRef(elements) => reference_ast(ty(&PlanType::Tuple(elements.clone()), span), true),
        PlanType::RangeValue => ast::TyKind::Tup(range_field_types().iter().map(|field| ty(field, span)).collect()),
        PlanType::Decimal => runtime_carrier_ty(RuntimeCarrier::Decimal, span),
        PlanType::FrozenStr => runtime_carrier_ty(RuntimeCarrier::FrozenStr, span),
        PlanType::FrozenBytes => runtime_carrier_ty(RuntimeCarrier::FrozenBytes, span),
        PlanType::ValidationError => runtime_carrier_ty(RuntimeCarrier::ValidationError, span),
        PlanType::ValidationErrorRef => reference_ast(ty(&PlanType::ValidationError, span), false),
        PlanType::ListIntoIter(leaf, depth) => list_iterator_signature_type(leaf, *depth, span),
        PlanType::ListIntoIterMutRef(leaf, depth) => {
            reference_ast(ty(&PlanType::ListIntoIter(leaf.clone(), *depth), span), true)
        }
        PlanType::Unit => ast::TyKind::Tup(ThinVec::new()),
        PlanType::Never => ast::TyKind::Never,
        PlanType::UnitFunction => unit_function_ast(span),
        PlanType::ModelRef(index, name) | PlanType::ModelMutRef(index, name) => reference_ast(
            ty(&PlanType::Model(*index, name.clone()), span),
            matches!(kind, PlanType::ModelMutRef(..)),
        ),
        PlanType::StringRef | PlanType::StringMutRef => {
            reference_ast(ty(&PlanType::String, span), matches!(kind, PlanType::StringMutRef))
        }
        PlanType::EnumRef(index, name) | PlanType::EnumMutRef(index, name) => reference_ast(
            ty(&PlanType::Enum(*index, name.clone()), span),
            matches!(kind, PlanType::EnumMutRef(..)),
        ),
        PlanType::CheckedNumeric(kind) => {
            ast::TyKind::Tup(thin_vec![numeric_ty(kind, span), ty(&PlanType::Bool, span)])
        }
        PlanType::FunctionPointer(signature) => function_pointer_ast(signature, span),
        PlanType::SharedCallable(signature) => ast::TyKind::Path(
            None,
            ast::Path::from_ident(ident(&crate::shared_callables::name(signature), span)),
        ),
        PlanType::Boxed(elements) => boxed_object_ast(
            elements
                .first()
                .map(|referent| ty(referent, span))
                .unwrap_or_else(|| ty(&PlanType::Unit, span)),
            span,
        ),
        PlanType::Closure(signature) | PlanType::ClosureOnce(signature) => boxed_object_ast(
            callable_object_ast(signature, span, matches!(kind, PlanType::ClosureOnce(_))),
            span,
        ),
        PlanType::NativeClosure(signature, send, sync) => {
            boxed_object_ast(native_callable_object_ast(signature, span, false, *send, *sync), span)
        }
        PlanType::NativeTraitObject(path, send, sync) => native_object_ast(path, *send, *sync, span),
        PlanType::ClosureRef(signature) => reference_ast(callable_object_ast(signature, span, false), false),
        PlanType::BorrowingClosure(signature) => boxed_object_ast(borrowing_callable_object_ast(signature, span), span),
        PlanType::BorrowingClosureRef(signature) => {
            reference_ast(borrowing_callable_object_ast(signature, span), false)
        }
        PlanType::CheckedInt => ast::TyKind::Tup(thin_vec![ty(&PlanType::Int, span), ty(&PlanType::Bool, span)]),
        other => primitive_or_nominal_ty(other, span),
    }
}

/// Preserve the supplied referent's span and tokens while declaring its exact reference mutability.
pub(super) fn reference_ast(referent: Box<ast::Ty>, mutable: bool) -> ast::TyKind {
    ast::TyKind::Ref(
        None,
        ast::MutTy {
            ty: referent,
            mutbl: if mutable {
                ast::Mutability::Mut
            } else {
                ast::Mutability::Not
            },
        },
    )
}

/// Declare the safe Rust function pointer used for a zero-argument unit callback.
pub(super) fn unit_function_ast(span: Span) -> ast::TyKind {
    ast::TyKind::FnPtr(Box::new(ast::FnPtrTy {
        safety: ast::Safety::Default,
        ext: ast::Extern::None,
        generic_params: ThinVec::new(),
        decl: Box::new(ast::FnDecl {
            inputs: ThinVec::new(),
            output: ast::FnRetTy::Ty(ty(&PlanType::Unit, span)),
        }),
        decl_span: span,
    }))
}

/// Place an already constructed callable object in the standard owning box without changing its bounds.
pub(super) fn boxed_object_ast(object: Box<ast::Ty>, span: Span) -> ast::TyKind {
    let mut path = boxed_path(span);
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(object))],
        })));
    }
    ast::TyKind::Path(None, path)
}

/// Declare the actual frozen wrapper with explicit static slice/text lifetimes, including nested collections.
pub(super) fn frozen_collection_ty(name: &str, elements: &[PlanType], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("incan_std_core", span));
    path.segments.push(ast::PathSegment::from_ident(ident("frozen", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    let arguments = elements
        .iter()
        .map(|element| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(element, span))))
        .collect();
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: arguments,
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        span,
        tokens: None,
        kind: ast::TyKind::Path(None, path),
    })
}

/// Declare the standard consuming iterator's immediate item type; positive nesting depth is validated before AST
/// construction.
pub(super) fn list_iterator_signature_type(leaf: &ListLeaf, depth: i64, span: Span) -> ast::TyKind {
    let element = if depth == 1 {
        list_leaf_type(leaf.clone())
    } else {
        PlanType::List(leaf.clone(), depth - 1)
    };
    let mut path = ast::Path::from_ident(ident("std", span));
    path.segments.push(ast::PathSegment::from_ident(ident("vec", span)));
    let mut segment = ast::PathSegment::from_ident(ident("IntoIter", span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&element, span)))],
    })));
    path.segments.push(segment);
    ast::TyKind::Path(None, path)
}

/// Declare retained static text and byte loans uniformly inside frozen carriers and ordinary generic frames.
pub(super) fn static_reference_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let inner = match kind {
        PlanType::StaticText => ast::TyKind::Path(None, ast::Path::from_ident(ident("str", span))),
        PlanType::StaticBytes => ast::TyKind::Slice(ty(&PlanType::U8, span)),
        _ => return None,
    };
    Some(Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        span,
        tokens: None,
        kind: ast::TyKind::Ref(
            Some(ast::Lifetime {
                id: ast::DUMMY_NODE_ID,
                ident: ident("'static", span),
            }),
            ast::MutTy {
                ty: Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    span,
                    tokens: None,
                    kind: inner,
                }),
                mutbl: ast::Mutability::Not,
            },
        ),
    }))
}

/// Name scalar primitives and crate-owned nominal aliases after their exact plan identity has been validated.
pub(super) fn primitive_or_nominal_ty(kind: &PlanType, span: Span) -> ast::TyKind {
    let name = match kind {
        PlanType::ISize => "isize",
        PlanType::USize => "usize",
        PlanType::Int => "i64",
        PlanType::Float => "f64",
        PlanType::I8 => "i8",
        PlanType::I16 => "i16",
        PlanType::I32 => "i32",
        PlanType::I128 => "i128",
        PlanType::U8 => "u8",
        PlanType::U16 => "u16",
        PlanType::U32 => "u32",
        PlanType::U64 => "u64",
        PlanType::U128 => "u128",
        PlanType::F32 => "f32",
        PlanType::F64 => "f64",
        PlanType::Model(_, name) | PlanType::Enum(_, name) => name.as_str(),
        _ => "bool",
    };
    let mut path = ast::Path::from_ident(ident(name, span));
    if matches!(kind, PlanType::Model(..) | PlanType::Enum(..)) {
        path = ast::Path::from_ident(ident("crate", span));
        path.segments.push(ast::PathSegment::from_ident(ident(name, span)));
    }
    ast::TyKind::Path(None, path)
}

/// Construct admitted owned and borrowed collections separately from scalar and nominal declarations.
pub(super) fn collection_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let kind = match kind {
        PlanType::List(leaf, depth) => {
            let mut element = ty(&list_leaf_type(leaf.clone()), span);
            for _ in 0..*depth {
                let mut path = ast::Path::from_ident(ident("Vec", span));
                path.segments[0].args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
                    span,
                    args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(element))],
                })));
                element = Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::TyKind::Path(None, path),
                    span,
                    tokens: None,
                });
            }
            return Some(element);
        }
        PlanType::ListRef(leaf, depth) | PlanType::ListMutRef(leaf, depth) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::List(leaf.clone(), *depth), span),
                mutbl: if matches!(kind, PlanType::ListMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::Set(leaf) => return Some(hashed_type("HashSet", &[leaf], span)),
        PlanType::Dict(key, value) => return Some(hashed_type("HashMap", &[key, value], span)),
        PlanType::SetRef(leaf) | PlanType::SetMutRef(leaf) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: hashed_type("HashSet", &[leaf], span),
                mutbl: if matches!(kind, PlanType::SetMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::DictRef(key, value) | PlanType::DictMutRef(key, value) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: hashed_type("HashMap", &[key, value], span),
                mutbl: if matches!(kind, PlanType::DictMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        _ => return None,
    };
    Some(Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        tokens: None,
    }))
}

/// Declare the runtime generator's canonical type without creating an alternative wrapper layout.
pub(super) fn generator_ty(name: &str, leaf: &ListLeaf, depth: i64, span: Span) -> Box<ast::Ty> {
    let element = if depth == 0 {
        list_leaf_type(leaf.clone())
    } else {
        PlanType::List(leaf.clone(), depth)
    };
    let mut path = ast::Path::from_ident(ident("incan_std_core", span));
    path.segments.push(ast::PathSegment::from_ident(ident("iter", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&element, span)))],
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Declare a checked nongeneric native principal and its explicit auto traits without parsing display syntax.
pub(super) fn native_object_ast(path: &str, send: bool, sync: bool, span: Span) -> ast::TyKind {
    let mut bounds = Vec::new();
    for path in std::iter::once(path)
        .chain(send.then_some("std::marker::Send"))
        .chain(sync.then_some("std::marker::Sync"))
    {
        let native = ast::Path {
            span,
            segments: std::iter::once(ast::PathSegment::path_root(span))
                .chain(
                    path.split("::")
                        .map(|segment| ast::PathSegment::from_ident(ident(segment, span))),
                )
                .collect(),
            tokens: None,
        };
        bounds.push(ast::GenericBound::Trait(ast::PolyTraitRef::new(
            ThinVec::new(),
            native,
            ast::TraitBoundModifiers::NONE,
            span,
            ast::Parens::No,
        )));
    }
    ast::TyKind::TraitObject(bounds, ast::TraitObjectSyntax::Dyn)
}

/// Select the private reusable Fn alias or construct the distinct consuming FnOnce object structurally.
/// The reusable alias retains rustc-owned bound-input lifetimes and is rooted at the crate so fields inside concrete
/// layout namespaces see the same object type. Consuming environments never acquire its clone ABI.
pub(crate) fn callable_object_ast(signature: &[PlanType], span: Span, consumes_environment: bool) -> Box<ast::Ty> {
    if !consumes_environment {
        let mut path = ast::Path::from_ident(ident("crate", span));
        path.segments.push(ast::PathSegment::from_ident(ident(
            &crate::cloneable_closures::alias_name(signature),
            span,
        )));
        return Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Path(None, path),
            span,
            tokens: None,
        });
    }

    native_callable_object_ast(signature, span, consumes_environment, false, false)
}

/// Construct the native Fn-family object and the auto traits proven for its owned captures, without a clone alias.
pub(super) fn native_callable_object_ast(
    signature: &[PlanType],
    span: Span,
    consumes_environment: bool,
    send: bool,
    sync: bool,
) -> Box<ast::Ty> {
    let (inputs, output) = match signature.split_last() {
        Some((result, parameters)) => (
            parameters.iter().map(|parameter| ty(parameter, span)).collect(),
            ty(result, span),
        ),
        None => (ThinVec::new(), ty(&PlanType::Unit, span)),
    };
    let mut path = ast::Path::from_ident(ident(if consumes_environment { "FnOnce" } else { "Fn" }, span));
    path.segments[0].args = Some(Box::new(ast::GenericArgs::Parenthesized(ast::ParenthesizedArgs {
        span,
        inputs,
        inputs_span: span,
        output: ast::FnRetTy::Ty(output),
    })));
    let bound = ast::GenericBound::Trait(ast::PolyTraitRef::new(
        ThinVec::new(),
        path,
        ast::TraitBoundModifiers::NONE,
        span,
        ast::Parens::No,
    ));
    let mut bounds = vec![bound];
    for (required, marker) in [(send, "Send"), (sync, "Sync")] {
        if required {
            let mut path = ast::Path::from_ident(ident("std", span));
            path.segments.push(ast::PathSegment::from_ident(ident("marker", span)));
            path.segments.push(ast::PathSegment::from_ident(ident(marker, span)));
            bounds.push(ast::GenericBound::Trait(ast::PolyTraitRef::new(
                ThinVec::new(),
                path,
                ast::TraitBoundModifiers::NONE,
                span,
                ast::Parens::No,
            )));
        }
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::TraitObject(bounds, ast::TraitObjectSyntax::Dyn),
        span,
        tokens: None,
    })
}

/// The Fn object of an environment holding borrowed references, outliving their inferred common bound.
pub(super) fn borrowing_callable_object_ast(signature: &[PlanType], span: Span) -> Box<ast::Ty> {
    borrowing_callable_object_ast_with_lifetime(signature, None, span)
}

/// Build a borrowing callable object whose common bound is inferred or named by its constructor.
fn borrowing_callable_object_ast_with_lifetime(
    signature: &[PlanType],
    lifetime: Option<&str>,
    span: Span,
) -> Box<ast::Ty> {
    let mut object = native_callable_object_ast(signature, span, false, false, false);
    if let ast::TyKind::TraitObject(bounds, _) = &mut object.kind {
        bounds.push(ast::GenericBound::Outlives(ast::Lifetime {
            id: ast::DUMMY_NODE_ID,
            ident: lifetime.map_or_else(
                || rustc_span::Ident::new(rustc_span::kw::UnderscoreLifetime, span),
                |name| rustc_span::Ident::new(rustc_span::Symbol::intern(name), span),
            ),
        }));
    }
    object
}

/// Build the boxed borrowing closure result of a constructor with one explicit common capture lifetime.
pub(crate) fn borrowing_closure_ast(signature: &[PlanType], lifetime: &str, span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: boxed_object_ast(
            borrowing_callable_object_ast_with_lifetime(signature, Some(lifetime), span),
            span,
        ),
        span,
        tokens: None,
    })
}

/// Construct a function-pointer signature structurally; preflight has proved a final return entry exists.
pub(super) fn function_pointer_ast(signature: &[PlanType], span: Span) -> ast::TyKind {
    let Some((result, parameters)) = signature.split_last() else {
        unreachable!("preflight rejects empty function-pointer signatures");
    };
    let inputs = parameters.iter().map(|parameter| ty(parameter, span)).collect();
    crate::ast_build::function_pointer(inputs, ty(result, span), span).kind
}

/// A diverging placeholder satisfies every admitted signature; `mir_built` replaces its body.
pub(super) fn placeholder(span: Span) -> Box<ast::Block> {
    let empty = Box::new(ast::Block {
        stmts: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    });
    let diverge = Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind: ast::ExprKind::Loop(empty, None, span),
        span,
        attrs: ThinVec::new(),
        tokens: None,
    });
    Box::new(ast::Block {
        stmts: thin_vec![ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Expr(diverge),
            span
        }],
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    })
}
