//! Declarations enter as AST items; only MIR supplies executable bodies.

use crate::plan::{Function, ListLeaf, PlanType, SizedNumeric, list_leaf_type, range_field_types, tuple_element_type};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Create an identifier from a name that preflight validation has already admitted.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Retain the standard library's absolute carrier identity independently of source nominal spellings.
fn standard_path(segments: &[&str], span: Span) -> ast::Path {
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
fn generator_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
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
fn scalar_reference_ast(elements: &[PlanType], mutable: bool, span: Span) -> ast::TyKind {
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
    let kind = match kind {
        PlanType::ScalarRef(elements) => scalar_reference_ast(elements, false, span),
        PlanType::ScalarMutRef(elements) => scalar_reference_ast(elements, true, span),
        PlanType::Future(arguments) => {
            let name = arguments.first().map(crate::futures::alias_name).unwrap_or_default();
            ast::TyKind::Path(None, ast::Path::from_ident(ident(&name, span)))
        }
        PlanType::String => ast::TyKind::Path(None, standard_path(&["string", "String"], span)),
        PlanType::NativeStr => ast::TyKind::Path(None, ast::Path::from_ident(ident("str", span))),
        PlanType::NativeSlice(elements) => elements
            .first()
            .map_or(ast::TyKind::Infer, |element| ast::TyKind::Slice(ty(element, span))),
        PlanType::SliceRef(elements) | PlanType::SliceMutRef(elements) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::NativeSlice(elements.clone()), span),
                mutbl: if matches!(kind, PlanType::SliceMutRef(_)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::CallableArguments(inputs) => ast::TyKind::Tup(inputs.iter().map(|input| ty(input, span)).collect()),
        PlanType::Tuple(elements) => ast::TyKind::Tup(
            elements
                .iter()
                .map(|element| ty(&tuple_element_type(element.clone()), span))
                .collect(),
        ),
        PlanType::TupleMutRef(elements) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::Tuple(elements.clone()), span),
                mutbl: ast::Mutability::Mut,
            },
        ),
        PlanType::RangeValue => ast::TyKind::Tup(range_field_types().iter().map(|field| ty(field, span)).collect()),
        PlanType::Decimal => {
            let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
            path.segments
                .push(ast::PathSegment::from_ident(ident("DecimalCarrier", span)));
            ast::TyKind::Path(None, path)
        }
        PlanType::FrozenStr => {
            let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
            path.segments
                .push(ast::PathSegment::from_ident(ident("FrozenCarrier", span)));
            ast::TyKind::Path(None, path)
        }
        PlanType::FrozenBytes => {
            let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
            path.segments
                .push(ast::PathSegment::from_ident(ident("FrozenBytesCarrier", span)));
            ast::TyKind::Path(None, path)
        }
        PlanType::ValidationError => {
            let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
            path.segments
                .push(ast::PathSegment::from_ident(ident("ValidationErrorCarrier", span)));
            ast::TyKind::Path(None, path)
        }
        PlanType::ValidationErrorRef => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::ValidationError, span),
                mutbl: ast::Mutability::Not,
            },
        ),
        PlanType::ListIntoIter(leaf, depth) => list_iterator_signature_type(leaf, *depth, span),
        PlanType::ListIntoIterMutRef(leaf, depth) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::ListIntoIter(leaf.clone(), *depth), span),
                mutbl: ast::Mutability::Mut,
            },
        ),
        PlanType::Unit => ast::TyKind::Tup(ThinVec::new()),
        PlanType::UnitFunction => ast::TyKind::FnPtr(Box::new(ast::FnPtrTy {
            safety: ast::Safety::Default,
            ext: ast::Extern::None,
            generic_params: ThinVec::new(),
            decl: Box::new(ast::FnDecl {
                inputs: ThinVec::new(),
                output: ast::FnRetTy::Ty(ty(&PlanType::Unit, span)),
            }),
            decl_span: span,
        })),
        PlanType::ModelRef(index, name) | PlanType::ModelMutRef(index, name) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::Model(*index, name.clone()), span),
                mutbl: if matches!(kind, PlanType::ModelMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::StringMutRef => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::String, span),
                mutbl: ast::Mutability::Mut,
            },
        ),
        PlanType::EnumRef(index, name) | PlanType::EnumMutRef(index, name) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: ty(&PlanType::Enum(*index, name.clone()), span),
                mutbl: if matches!(kind, PlanType::EnumMutRef(..)) {
                    ast::Mutability::Mut
                } else {
                    ast::Mutability::Not
                },
            },
        ),
        PlanType::CheckedNumeric(kind) => {
            ast::TyKind::Tup(thin_vec![numeric_ty(kind, span), ty(&PlanType::Bool, span)])
        }
        PlanType::FunctionPointer(signature) => function_pointer_ast(signature, span),
        PlanType::SharedCallable(signature) => ast::TyKind::Path(
            None,
            ast::Path::from_ident(ident(&crate::shared_callables::name(signature), span)),
        ),
        PlanType::Closure(signature) | PlanType::ClosureOnce(signature) => {
            let mut path = boxed_path(span);
            if let Some(segment) = path.segments.last_mut() {
                segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
                    span,
                    args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(callable_object_ast(
                        signature,
                        span,
                        matches!(kind, PlanType::ClosureOnce(_))
                    )))],
                })));
            }
            ast::TyKind::Path(None, path)
        }
        PlanType::ClosureRef(signature) => ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: callable_object_ast(signature, span, false),
                mutbl: ast::Mutability::Not,
            },
        ),
        PlanType::CheckedInt => ast::TyKind::Tup(thin_vec![ty(&PlanType::Int, span), ty(&PlanType::Bool, span)]),
        other => primitive_or_nominal_ty(other, span),
    };
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        tokens: None,
    })
}

/// Declare the actual frozen wrapper with explicit static slice/text lifetimes, including nested collections.
fn frozen_collection_ty(name: &str, elements: &[PlanType], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("incan_std_core", span));
    path.segments.push(ast::PathSegment::from_ident(ident("frozen", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    let arguments = elements
        .iter()
        .map(|element| {
            let input = match element {
                PlanType::StaticText | PlanType::StaticBytes => {
                    let inner = if matches!(element, PlanType::StaticText) {
                        ast::TyKind::Path(None, ast::Path::from_ident(ident("str", span)))
                    } else {
                        ast::TyKind::Slice(ty(&PlanType::U8, span))
                    };
                    Box::new(ast::Ty {
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
                    })
                }
                _ => ty(element, span),
            };
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(input))
        })
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
fn list_iterator_signature_type(leaf: &ListLeaf, depth: i64, span: Span) -> ast::TyKind {
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

/// Name scalar primitives and crate-owned nominal aliases after their exact plan identity has been validated.
fn primitive_or_nominal_ty(kind: &PlanType, span: Span) -> ast::TyKind {
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
fn collection_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
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
fn generator_ty(name: &str, leaf: &ListLeaf, depth: i64, span: Span) -> Box<ast::Ty> {
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

/// Select the private reusable Fn alias or construct the distinct consuming FnOnce object structurally.
/// The reusable alias retains rustc-owned bound-input lifetimes; consuming environments never acquire its clone ABI.
pub(crate) fn callable_object_ast(signature: &[PlanType], span: Span, consumes_environment: bool) -> Box<ast::Ty> {
    if !consumes_environment {
        return Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Path(
                None,
                ast::Path::from_ident(ident(&crate::cloneable_closures::alias_name(signature), span)),
            ),
            span,
            tokens: None,
        });
    }

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
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::TraitObject(vec![bound], ast::TraitObjectSyntax::Dyn),
        span,
        tokens: None,
    })
}

/// Construct a function-pointer signature structurally; preflight has proved a final return entry exists.
fn function_pointer_ast(signature: &[PlanType], span: Span) -> ast::TyKind {
    let Some((result, parameters)) = signature.split_last() else {
        unreachable!("preflight rejects empty function-pointer signatures");
    };
    let inputs = parameters.iter().map(|parameter| ty(parameter, span)).collect();
    crate::ast_build::function_pointer(inputs, ty(result, span), span).kind
}

/// A diverging placeholder satisfies every admitted signature; `mir_built` replaces its body.
fn placeholder(span: Span) -> Box<ast::Block> {
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

/// Inject one source-named function's declaration with no authored body in the AST.
pub fn function(function: &Function, span: Span) -> Box<ast::Item> {
    let inputs = function
        .parameters
        .iter()
        .map(|parameter| ast::Param {
            attrs: ThinVec::new(),
            ty: ty(&parameter.ty, span),
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(&parameter.name, span), None),
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let output = ast::FnRetTy::Ty(
        crate::futures::constructor_return_type(function, span).unwrap_or_else(|| ty(&function.return_type, span)),
    );
    let signature = ast::FnSig {
        header: ast::FnHeader {
            coroutine_kind: function.is_async.then_some(ast::CoroutineKind::Async {
                span,
                closure_id: ast::DUMMY_NODE_ID,
                return_impl_trait_id: ast::DUMMY_NODE_ID,
            }),
            ..ast::FnHeader::default()
        },
        decl: Box::new(ast::FnDecl { inputs, output }),
        span,
    };
    item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&function.name, span),
            generics: ast::Generics::default(),
            sig: signature,
            contract: None,
            define_opaque: None,
            body: Some(
                crate::captured_generators::declaration_body(function, span)
                    .or_else(|| crate::closures::declaration_body(function, span))
                    .or_else(|| crate::shared_callables::declaration_body(function, span))
                    .or_else(|| crate::futures::declaration_body(function, span))
                    .unwrap_or_else(|| placeholder(span)),
            ),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Build an item without source tokens; rustc assigns node identities after injection.
pub(crate) fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: ast::Visibility {
            kind: ast::VisibilityKind::Public,
            span,
            tokens: None,
        },
        kind,
        tokens: None,
    })
}

/// Load each admitted external crate so canonical path resolution sees its module tree.
pub fn external_crate(name: &str, span: Span) -> Box<ast::Item> {
    item(ast::ItemKind::ExternCrate(None, ident(name, span)), span)
}

/// Name the private namespace containing one source-named concrete layout; preflight checks its root collision.
pub(crate) fn model_module_name(model: &crate::plan::ModelDeclaration) -> String {
    format!("__incan_layout_{}", model.name)
}

/// Keep a concrete struct's source name for derives, exposing its unique physical symbol through a type alias.
/// Ordinary layouts stay at the crate root. Specialized layouts require no executable adapter body: Rust's derives
/// operate on a source-named struct in a private namespace, and MIR uses the alias's identical underlying ADT.
pub(crate) fn model_items(
    model: &crate::plan::ModelDeclaration,
    generator: &ast::attr::AttrIdGenerator,
    span: Span,
) -> Vec<Box<ast::Item>> {
    if !model.library_item.is_empty() {
        return vec![library_alias(&model.name, model.public, span)];
    }
    let mut declaration = self::model(model, span);
    for derive in model.derives.iter().filter(|derive| derive.as_str() != "Display") {
        declaration.attrs.push(derive_attribute(generator, derive, span));
    }
    if !model.checked_deserialize.is_empty() {
        declaration
            .attrs
            .push(crate::checked_json::deserialize_attribute(generator, span));
    }
    if !specialized_model(model) {
        return vec![declaration];
    }
    let namespace = model_module_name(model);
    let module = item(
        ast::ItemKind::Mod(
            ast::Safety::Default,
            ident(&namespace, span),
            ast::ModKind::Loaded(
                thin_vec![declaration],
                ast::Inline::Yes,
                ast::ModSpans {
                    inner_span: span,
                    inject_use_span: span,
                },
            ),
        ),
        span,
    );
    let mut path = ast::Path::from_ident(ident(&namespace, span));
    path.segments
        .push(ast::PathSegment::from_ident(ident(&model.source_name, span)));
    let mut alias = item(
        ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&model.name, span),
            generics: ast::Generics::default(),
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(Box::new(ast::Ty {
                id: ast::DUMMY_NODE_ID,
                kind: ast::TyKind::Path(None, path),
                span,
                tokens: None,
            })),
        })),
        span,
    );
    alias.vis = visibility(model.public, span);
    vec![module, alias]
}

/// Declare a native layout's alias shell. The adapter supplies its exact metadata type through `type_of`, including
/// erased Rust lifetime arguments. Reconstructing its target in AST would create a second generic binding contract
/// and require source lifetime binders that are deliberately absent from Body IR.
fn library_alias(name: &str, public: bool, span: Span) -> Box<ast::Item> {
    let mut alias = item(
        ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            after_where_clause: ast::WhereClause::default(),
            bounds: Vec::new(),
            ty: Some(Box::new(ast::Ty {
                id: ast::DUMMY_NODE_ID,
                kind: ast::TyKind::Tup(ThinVec::new()),
                span,
                tokens: None,
            })),
        })),
        span,
    );
    alias.vis = visibility(public, span);
    alias
}

/// Distinguish specialized layouts from legacy plan records whose source name defaults to their physical name.
fn specialized_model(model: &crate::plan::ModelDeclaration) -> bool {
    !model.source_name.is_empty() && model.source_name != model.name
}

/// Choose the validated source identifier only when a distinct concrete layout alias is retained.
fn model_source_name(model: &crate::plan::ModelDeclaration) -> &str {
    if specialized_model(model) {
        &model.source_name
    } else {
        &model.name
    }
}

/// Inject the checked layout and field order; a specialized struct is public inside its private namespace.
pub fn model(model: &crate::plan::ModelDeclaration, span: Span) -> Box<ast::Item> {
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    let fields = model
        .fields
        .iter()
        .zip(&model.field_public)
        .map(|(field, public)| ast::FieldDef {
            attrs: ThinVec::new(),
            id: ast::DUMMY_NODE_ID,
            span,
            vis: visibility(*public, span),
            mut_restriction: ast::MutRestriction {
                kind: ast::RestrictionKind::Unrestricted,
                span,
                tokens: None,
            },
            safety: ast::Safety::Default,
            ident: if tuple { None } else { Some(ident(&field.name, span)) },
            ty: ty(&field.ty, span),
            default: None,
            is_placeholder: false,
        })
        .collect();
    let mut declaration = item(
        ast::ItemKind::Struct(
            ident(model_source_name(model), span),
            ast::Generics::default(),
            if tuple {
                ast::VariantData::Tuple(fields, ast::DUMMY_NODE_ID)
            } else {
                ast::VariantData::Struct {
                    fields,
                    recovered: ast::Recovered::No,
                }
            },
        ),
        span,
    );
    declaration.vis = visibility(model.public || specialized_model(model), span);
    declaration.tokens = Some(model_tokens(model, span));
    declaration
}

/// Retain the injected declaration's tokens for procedural derives, without parsing or generating source text.
fn model_tokens(model: &crate::plan::ModelDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut fields = Vec::new();
    let tuple = model.fields.len() == 1 && model.fields[0].name == "0";
    for (slot, (field, public)) in model.fields.iter().zip(&model.field_public).enumerate() {
        if let Some(serde_name) = model.field_serde_names.get(slot).filter(|name| !name.is_empty()) {
            fields.extend(serde_rename_tokens(serde_name, span));
        }
        if *public {
            fields.push(keyword_token("pub", span));
        }
        if !tuple {
            fields.push(name_token(&field.name, span));
            fields.push(AttrTokenTree::Token(
                ast::token::Token::new(TokenKind::Colon, span),
                Spacing::Alone,
            ));
        }
        fields.extend(type_tokens(&ty(&field.ty, span)));
        fields.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Comma, span),
            Spacing::Alone,
        ));
    }
    let mut tokens = Vec::new();
    if model.public || specialized_model(model) {
        tokens.push(keyword_token("pub", span));
    }
    tokens.push(keyword_token("struct", span));
    tokens.push(name_token(model_source_name(model), span));
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        if tuple {
            Delimiter::Parenthesis
        } else {
            Delimiter::Brace
        },
        AttrTokenStream::new(fields),
    ));
    if tuple {
        tokens.push(AttrTokenTree::Token(
            ast::token::Token::new(TokenKind::Semi, span),
            Spacing::Alone,
        ));
    }
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}

/// `#[serde(rename = "name")]` before a field in the derive input, as legacy writes it for an aliased field.
///
/// The attribute exists only for serde's derive, which reads it as its helper; the injected AST field needs none.
fn serde_rename_tokens(name: &str, span: Span) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Lit, LitKind, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};
    let literal = Lit::new(LitKind::Str, Symbol::intern(&name.escape_default().to_string()), None);
    let arguments = AttrTokenStream::new(vec![
        name_token("rename", span),
        AttrTokenTree::Token(Token::new(TokenKind::Eq, span), Spacing::Alone),
        AttrTokenTree::Token(Token::new(TokenKind::Literal(literal), span), Spacing::Alone),
    ]);
    vec![
        AttrTokenTree::Token(Token::new(TokenKind::Pound, span), Spacing::JointHidden),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::JointHidden, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(vec![
                name_token("serde", span),
                AttrTokenTree::Delimited(
                    DelimSpan::from_single(span),
                    DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                    Delimiter::Parenthesis,
                    arguments,
                ),
            ]),
        ),
    ]
}

/// Turn an admitted AST identifier into a token with the same source span and hygiene.
fn name_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::from_ast_ident(ident(name, span)),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Emit declaration keywords as keywords rather than the raw identifiers inferred for reserved AST names.
fn keyword_token(name: &str, span: Span) -> ast::tokenstream::AttrTokenTree {
    ast::tokenstream::AttrTokenTree::Token(
        ast::token::Token::new(
            ast::token::TokenKind::Ident(Symbol::intern(name), ast::token::IdentIsRaw::No),
            span,
        ),
        ast::tokenstream::Spacing::Alone,
    )
}

/// Mirror admitted paths, tuples, references, slices, and function-pointer signatures into procedural derive tokens
/// while preserving explicit lifetimes.
fn type_tokens(ty: &ast::Ty) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};
    match &ty.kind {
        ast::TyKind::Path(None, path) => {
            let mut tokens = Vec::new();
            for (index, segment) in path.segments.iter().enumerate() {
                if segment.ident.name == rustc_span::kw::PathRoot {
                    continue;
                }
                if index != 0 {
                    tokens.push(AttrTokenTree::Token(
                        Token::new(TokenKind::PathSep, ty.span),
                        Spacing::Alone,
                    ));
                }
                tokens.push(name_token(segment.ident.name.as_str(), ty.span));
                if let Some(arguments) = &segment.args {
                    tokens.extend(generic_argument_tokens(arguments, ty.span));
                }
            }
            tokens
        }
        ast::TyKind::Tup(elements) => {
            let mut tokens = Vec::new();
            for element in elements {
                tokens.extend(type_tokens(element));
                tokens.push(AttrTokenTree::Token(
                    Token::new(TokenKind::Comma, ty.span),
                    Spacing::Alone,
                ));
            }
            vec![AttrTokenTree::Delimited(
                DelimSpan::from_single(ty.span),
                DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                Delimiter::Parenthesis,
                AttrTokenStream::new(tokens),
            )]
        }
        ast::TyKind::Ref(lifetime, referent) => {
            let mut tokens = vec![AttrTokenTree::Token(
                Token::new(TokenKind::And, ty.span),
                Spacing::Alone,
            )];
            if let Some(lifetime) = lifetime {
                tokens.push(AttrTokenTree::Token(
                    Token::new(
                        TokenKind::Lifetime(lifetime.ident.name, ast::token::IdentIsRaw::No),
                        ty.span,
                    ),
                    Spacing::Alone,
                ));
            }
            if referent.mutbl == ast::Mutability::Mut {
                tokens.push(keyword_token("mut", ty.span));
            }
            tokens.extend(type_tokens(&referent.ty));
            tokens
        }
        ast::TyKind::FnPtr(pointer) => function_pointer_tokens(pointer, ty.span),
        ast::TyKind::Slice(element) => vec![AttrTokenTree::Delimited(
            DelimSpan::from_single(ty.span),
            DelimSpacing::new(Spacing::Alone, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(type_tokens(element)),
        )],
        _ => {
            unreachable!("the admitted AST type builder emits paths, tuples, references, slices, and function pointers")
        }
    }
}

/// Mirror the safe, ordinary function-pointer AST builder's exact inputs and output without inventing token types.
/// Borrowed signature components recurse through the same reference and container token projection as other fields.
fn function_pointer_tokens(pointer: &ast::FnPtrTy, span: Span) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, Spacing};

    // ---- Signature inputs ----
    let mut parameters = Vec::new();
    for parameter in &pointer.decl.inputs {
        parameters.extend(type_tokens(&parameter.ty));
        parameters.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
    }
    let mut tokens = vec![
        keyword_token("fn", span),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::Alone, Spacing::Alone),
            Delimiter::Parenthesis,
            AttrTokenStream::new(parameters),
        ),
    ];

    // ---- Signature output ----
    if let ast::FnRetTy::Ty(output) = &pointer.decl.output {
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::RArrow, span),
            Spacing::Alone,
        ));
        tokens.extend(type_tokens(output));
    }
    tokens
}

/// Mirror a path segment's type arguments (`Vec<Tag>`, `HashMap<String, i64>`) into derive input tokens.
///
/// Without them a derive would name a generic struct bare and fail to compile; the admitted type builder spells only
/// type arguments, never lifetimes or constants.
fn generic_argument_tokens(arguments: &ast::GenericArgs, span: Span) -> Vec<ast::tokenstream::AttrTokenTree> {
    use ast::token::{Token, TokenKind};
    use ast::tokenstream::{AttrTokenTree, Spacing};
    let ast::GenericArgs::AngleBracketed(angle) = arguments else {
        unreachable!("the admitted AST type builder emits only angle-bracketed type arguments")
    };
    let mut tokens = vec![AttrTokenTree::Token(Token::new(TokenKind::Lt, span), Spacing::Alone)];
    for (index, argument) in angle.args.iter().enumerate() {
        if index != 0 {
            tokens.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
        }
        match argument {
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(argument)) => tokens.extend(type_tokens(argument)),
            _ => unreachable!("the admitted AST type builder emits only type arguments"),
        }
    }
    tokens.push(AttrTokenTree::Token(Token::new(TokenKind::Gt, span), Spacing::Alone));
    tokens
}

/// Convert checked visibility without widening private fields.
fn visibility(public: bool, span: Span) -> ast::Visibility {
    ast::Visibility {
        kind: if public {
            ast::VisibilityKind::Public
        } else {
            ast::VisibilityKind::Inherited
        },
        span,
        tokens: None,
    }
}

/// Build derive path tokens directly from the admitted registry names, including compiler-owned proc macros.
pub fn derive_attribute(generator: &ast::attr::AttrIdGenerator, name: &str, span: Span) -> ast::Attribute {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{
        AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing, TokenStream,
    };
    let mut tokens = Vec::new();
    if name.starts_with("serde::") {
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::PathSep, span),
            Spacing::Alone,
        ));
    }
    if matches!(name, "FieldInfo" | "IncanClass") {
        tokens.push(AttrTokenTree::Token(
            Token::from_ast_ident(ident("incan_derive", span)),
            Spacing::Alone,
        ));
        tokens.push(AttrTokenTree::Token(
            Token::new(TokenKind::PathSep, span),
            Spacing::Alone,
        ));
    }
    for (index, segment) in name.split("::").enumerate() {
        if index != 0 {
            tokens.push(AttrTokenTree::Token(
                Token::new(TokenKind::PathSep, span),
                Spacing::Alone,
            ));
        }
        tokens.push(AttrTokenTree::Token(
            Token::from_ast_ident(ident(segment, span)),
            Spacing::Alone,
        ));
    }
    let arguments = AttrTokenStream::new(tokens);
    let attribute_tokens = LazyAttrTokenStream::new_direct(AttrTokenStream::new(vec![
        AttrTokenTree::Token(Token::new(TokenKind::Pound, span), Spacing::JointHidden),
        AttrTokenTree::Delimited(
            DelimSpan::from_single(span),
            DelimSpacing::new(Spacing::JointHidden, Spacing::Alone),
            Delimiter::Bracket,
            AttrTokenStream::new(vec![
                name_token("derive", span),
                AttrTokenTree::Delimited(
                    DelimSpan::from_single(span),
                    DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                    Delimiter::Parenthesis,
                    arguments.clone(),
                ),
            ]),
        ),
    ]));
    ast::attr::mk_attr_from_item(
        generator,
        ast::AttrItem {
            unsafety: ast::Safety::Default,
            path: ast::Path::from_ident(ident("derive", span)),
            args: ast::AttrItemKind::Unparsed(ast::AttrArgs::Delimited(ast::DelimArgs {
                dspan: DelimSpan::from_single(span),
                delim: Delimiter::Parenthesis,
                tokens: TokenStream::new(arguments.to_token_trees()),
            })),
            tokens: None,
        },
        Some(attribute_tokens),
        ast::AttrStyle::Outer,
        span,
    )
}

/// Construct the type of an admitted overflow-pair value.
fn numeric_ty(kind: &SizedNumeric, span: Span) -> Box<ast::Ty> {
    let kind = match kind {
        SizedNumeric::I8 => PlanType::I8,
        SizedNumeric::I16 => PlanType::I16,
        SizedNumeric::I32 => PlanType::I32,
        SizedNumeric::I128 => PlanType::I128,
        SizedNumeric::U8 => PlanType::U8,
        SizedNumeric::U16 => PlanType::U16,
        SizedNumeric::U32 => PlanType::U32,
        SizedNumeric::U64 => PlanType::U64,
        SizedNumeric::U128 => PlanType::U128,
        SizedNumeric::F32 => PlanType::F32,
        SizedNumeric::F64 => PlanType::F64,
        SizedNumeric::ISize => PlanType::ISize,
        SizedNumeric::USize => PlanType::USize,
    };
    ty(&kind, span)
}

/// Build the standard collection path with explicit checked primitive type arguments.
fn hashed_type(name: &str, leaves: &[&ListLeaf], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("std", span));
    path.segments
        .push(ast::PathSegment::from_ident(ident("collections", span)));
    let mut segment = ast::PathSegment::from_ident(ident(name, span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: leaves
            .iter()
            .map(|leaf| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&list_leaf_type((*leaf).clone()), span))))
            .collect(),
    })));
    path.segments.push(segment);
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// Inject source-ordered unit and tuple variants as AST nodes; executable bodies still enter only through MIR.
pub fn enum_declaration(value: &crate::plan::EnumDeclaration, span: Span) -> Box<ast::Item> {
    if !value.carrier.is_empty() {
        return library_alias(&value.name, value.public, span);
    }
    if !value.library_item.is_empty() {
        return library_alias(&value.name, value.public, span);
    }
    let variants = value
        .variants
        .iter()
        .map(|variant| {
            let fields = variant
                .fields
                .iter()
                .map(|field| ast::FieldDef {
                    attrs: ThinVec::new(),
                    id: ast::DUMMY_NODE_ID,
                    span,
                    vis: visibility(false, span),
                    mut_restriction: ast::MutRestriction {
                        kind: ast::RestrictionKind::Unrestricted,
                        span,
                        tokens: None,
                    },
                    safety: ast::Safety::Default,
                    ident: None,
                    ty: ty(field, span),
                    default: None,
                    is_placeholder: false,
                })
                .collect();
            ast::Variant {
                attrs: ThinVec::new(),
                id: ast::DUMMY_NODE_ID,
                span,
                vis: visibility(false, span),
                ident: ident(&variant.name, span),
                data: if variant.fields.is_empty() {
                    ast::VariantData::Unit(ast::DUMMY_NODE_ID)
                } else {
                    ast::VariantData::Tuple(fields, ast::DUMMY_NODE_ID)
                },
                disr_expr: None,
                is_placeholder: false,
            }
        })
        .collect();
    let mut declaration = item(
        ast::ItemKind::Enum(
            ident(&value.name, span),
            ast::Generics::default(),
            ast::EnumDef { variants },
        ),
        span,
    );
    declaration.vis = visibility(value.public, span);
    declaration.tokens = Some(enum_tokens(value, span));
    declaration
}

/// Preserve enum tokens for derives from the exact planned layout without parsing generated Rust source.
fn enum_tokens(value: &crate::plan::EnumDeclaration, span: Span) -> ast::tokenstream::LazyAttrTokenStream {
    use ast::token::{Delimiter, Token, TokenKind};
    use ast::tokenstream::{AttrTokenStream, AttrTokenTree, DelimSpacing, DelimSpan, LazyAttrTokenStream, Spacing};
    let mut variants = Vec::new();
    for variant in &value.variants {
        variants.push(name_token(&variant.name, span));
        if !variant.fields.is_empty() {
            let mut fields = Vec::new();
            for field in &variant.fields {
                fields.extend(type_tokens(&ty(field, span)));
                fields.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
            }
            variants.push(AttrTokenTree::Delimited(
                DelimSpan::from_single(span),
                DelimSpacing::new(Spacing::Alone, Spacing::Alone),
                Delimiter::Parenthesis,
                AttrTokenStream::new(fields),
            ));
        }
        variants.push(AttrTokenTree::Token(Token::new(TokenKind::Comma, span), Spacing::Alone));
    }
    let mut tokens = Vec::new();
    if value.public {
        tokens.push(keyword_token("pub", span));
    }
    tokens.push(keyword_token("enum", span));
    tokens.push(name_token(&value.name, span));
    tokens.push(AttrTokenTree::Delimited(
        DelimSpan::from_single(span),
        DelimSpacing::new(Spacing::Alone, Spacing::Alone),
        Delimiter::Brace,
        AttrTokenStream::new(variants),
    ));
    LazyAttrTokenStream::new_direct(AttrTokenStream::new(tokens))
}

/// Declare the Incan runtime wrapper and its retained shared or exclusive reference without inventing its fields.
fn zip_signature_ty(kind: &PlanType, span: Span) -> Option<Box<ast::Ty>> {
    let (left, right, mutability) = match kind {
        PlanType::ZipIterator(left, right) => (left, right, None),
        PlanType::ZipIteratorRef(left, right) => (left, right, Some(ast::Mutability::Not)),
        PlanType::ZipIteratorMutRef(left, right) => (left, right, Some(ast::Mutability::Mut)),
        _ => return None,
    };
    let mut path = ast::Path::from_ident(ident("incan_native_runtime", span));
    let mut segment = ast::PathSegment::from_ident(ident("ZipLists", span));
    segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
        span,
        args: thin_vec![
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&tuple_element_type(left.clone()), span))),
            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty(&tuple_element_type(right.clone()), span))),
        ],
    })));
    path.segments.push(segment);
    let owned = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    });
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

/// Declare the SDK's type marker directly rather than inventing a source model or surrogate storage.
fn type_token_ty(elements: &[PlanType], span: Span) -> Box<ast::Ty> {
    let mut path = ast::Path::from_ident(ident("incan_std_core", span));
    path.segments
        .push(ast::PathSegment::from_ident(ident("reflection", span)));
    let mut segment = ast::PathSegment::from_ident(ident("TypeToken", span));
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
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}
