//! Rules the language reference states that the checker did not apply, each one either accepted where the contract
//! refuses it or refused where the contract accepts it (#1561): namespace `prelude` imports, `import python`, `import
//! M::X` of a private declaration, pattern arity, named arguments to `str` methods, writes through read-only bindings,
//! escaping C text views, JSON members with no JSON form, and `{value:?}` over a value with no `Debug` form.

use super::*;

/// Messages of `errors`, for assertion output.
fn messages(errors: &[CompileError]) -> Vec<&str> {
    errors.iter().map(|error| error.message.as_str()).collect()
}

/// Check `source` against an SDK catalog whose provider claims `modules` (below `std`) and nothing else.
fn check_with_sdk_modules(source: &str, modules: &[&[&str]]) -> Vec<CompileError> {
    let ast = parse_program(source, "sdk catalog consumer");
    let plan = ProviderPlan::for_in_memory_sdk_modules(
        LibraryManifestIndex::default(),
        modules
            .iter()
            .map(|module| module.iter().map(|segment| (*segment).to_string()).collect::<Vec<_>>()),
    );
    let mut checker = TypeChecker::new();
    checker.set_provider_plan(Arc::new(plan));
    checker.check_program(&ast).err().unwrap_or_default()
}

/// #1561: `std.async.prelude` and `std.traits.prelude` are the namespaces' own modules, so an SDK whose providers claim
/// `std.async` and `std.traits` resolves them, by `from` import and by module import, exactly as the namespace
/// spelling.
#[test]
fn namespace_prelude_imports_resolve_through_the_namespace_module_issue1561() -> Result<(), String> {
    let modules: &[&[&str]] = &[&["async"], &["traits"]];
    let prelude = check_with_sdk_modules(
        "from std.async.prelude import sleep, spawn\nfrom std.traits.prelude import Error\nimport std.async.prelude\n",
        modules,
    );
    let namespace = check_with_sdk_modules(
        "from std.async import sleep, spawn\nfrom std.traits import Error\nimport std.async\n",
        modules,
    );
    if prelude
        .iter()
        .any(|error| error.message.contains("Unknown stdlib module"))
    {
        return Err(format!(
            "a namespace prelude path must name the namespace module, got {:?}",
            messages(&prelude)
        ));
    }
    assert_eq!(
        prelude.len(),
        namespace.len(),
        "the prelude spelling must check like the namespace spelling: {:?} vs {:?}",
        messages(&prelude),
        messages(&namespace)
    );
    assert_check_ok(
        "import std.async.prelude\n\nasync def work() -> int:\n    return 1\n\nasync def main() -> None:\n    handle = prelude.spawn(work())\n    match await handle:\n        Ok(value) => println(value)\n        Err(_) => println(\"failed\")\n",
    );
    let root_prelude = check_with_sdk_modules("import std.prelude\n", &[&["prelude"]]);
    assert!(
        !root_prelude
            .iter()
            .any(|error| error.message.contains("Unknown stdlib module")),
        "the root std.prelude stays a module of its own, got {:?}",
        messages(&root_prelude)
    );
    Ok(())
}

/// #1561: `std.derives` has no module of its own, and an SDK catalog claims only the modules below it, as the compiled
/// SDK does. `from std.derives import comparison` imports the submodule, whose trait an adopter names through it,
/// with the catalog as with none; an item of `std.derives` that is not a submodule is refused as not exported in both,
/// not as an unknown module.
#[test]
fn stdlib_namespace_without_a_module_of_its_own_imports_its_submodules_issue1561() {
    let modules: &[&[&str]] = &[&["derives", "comparison"], &["derives", "copying"]];
    let adopter = "from std.derives import comparison\n\n\nmodel Qualified with comparison.Ord:\n    v: int\n\n    def __eq__(self, other: Self) -> bool:\n        return self.v == other.v\n\n    def __lt__(self, other: Self) -> bool:\n        return self.v < other.v\n\n\ndef main() -> None:\n    println(Qualified(v=1) < Qualified(v=2))\n";
    let accepted = check_with_sdk_modules(adopter, modules);
    assert!(
        accepted.is_empty(),
        "a submodule imported from std.derives must check against the SDK catalog, got {:?}",
        messages(&accepted)
    );
    assert_check_ok(adopter);

    let refused = check_with_sdk_modules("from std.derives import Eq\n", modules);
    assert!(
        refused.iter().any(|error| error
            .message
            .contains("Cannot import `Eq` from stdlib module `std.derives`: it is not exported by that module"))
            && !refused
                .iter()
                .any(|error| error.message.contains("Unknown stdlib module")),
        "a non-submodule item of std.derives must be refused as not exported, got {:?}",
        messages(&refused)
    );
    let refused_without_catalog = check_str_err("from std.derives import Eq\n", "std.derives has no members");
    assert!(
        refused_without_catalog.iter().any(|error| error
            .message
            .contains("Cannot import `Eq` from stdlib module `std.derives`: it is not exported by that module")),
        "without a catalog a non-submodule item of std.derives must be refused as not exported, got {:?}",
        messages(&refused_without_catalog)
    );
}

/// #1561: `import python "pkg"` names nothing the compiler provides, so the check refuses it instead of binding a name
/// the build cannot resolve.
#[test]
fn python_import_is_refused_issue1561() {
    let errors = check_str_err(
        "import python \"requests\" as pyreq\n\ndef main() -> None:\n    pass\n",
        "a python import must be refused",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("import python \"requests\"") && error.message.contains("Python")),
        "expected the python-import refusal, got {:?}",
        messages(&errors)
    );
}

/// #1561: `import M::X` binds the declaration `X` like `from M import X`, so it requires `M` to export it: a declaration
/// without `pub` is refused under both spellings, and a `pub` one is accepted under both.
#[test]
fn module_path_import_of_a_private_declaration_is_refused_issue1561() -> Result<(), String> {
    let helpers = parse_program(
        "pub def shown() -> int:\n    return 1\n\ndef hidden() -> int:\n    return 2\n",
        "helpers module",
    );
    for (spelling, source) in [
        (
            "from",
            "from helpers import hidden\n\ndef main() -> int:\n    return hidden()\n",
        ),
        (
            "module path",
            "import helpers::hidden\n\ndef main() -> int:\n    return hidden()\n",
        ),
        (
            "dotted module path",
            "import helpers.hidden\n\ndef main() -> int:\n    return hidden()\n",
        ),
    ] {
        let ast = parse_program(source, spelling);
        let mut checker = TypeChecker::new();
        let errors = checker
            .check_with_imports(&ast, &[("helpers", &helpers)])
            .err()
            .unwrap_or_default();
        if !errors
            .iter()
            .any(|error| error.message.contains("Cannot import `hidden`"))
        {
            return Err(format!(
                "{spelling}: a private declaration must be refused, got {:?}",
                messages(&errors)
            ));
        }
    }
    for source in [
        "from helpers import shown\n\ndef main() -> int:\n    return shown()\n",
        "import helpers::shown\n\ndef main() -> int:\n    return shown()\n",
        "import helpers\n\ndef main() -> int:\n    return helpers.shown()\n",
    ] {
        let ast = parse_program(source, "public import");
        let mut checker = TypeChecker::new();
        checker
            .check_with_imports(&ast, &[("helpers", &helpers)])
            .map_err(|errors| format!("a public declaration must import: {:?}", messages(&errors)))?;
    }
    Ok(())
}

/// #1561: a variant pattern has one sub-pattern per payload value and a tuple pattern one per element; fewer or more
/// is refused, the exact count is accepted.
#[test]
fn pattern_sub_pattern_count_must_match_issue1561() {
    let declarations = "enum Shape:\n    Circle(float)\n    Rect(float, float)\n    Empty\n\n";
    for (case, arm) in [
        ("variant with too few", "Shape.Rect(w) => return w"),
        ("variant with too many", "Shape.Circle(r, s) => return r"),
        ("payload variant with none", "Shape.Circle => return 0.0"),
    ] {
        let source = format!(
            "{declarations}def area(shape: Shape) -> float:\n    match shape:\n        {arm}\n        _ => return 0.0\n"
        );
        let errors = check_str_err(&source, case);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("sub-pattern") && error.message.contains("payload value")),
            "{case}: expected a pattern arity refusal, got {:?}",
            messages(&errors)
        );
    }
    for (case, pattern) in [
        ("tuple with too few", "(a, b)"),
        ("tuple with too many", "(a, b, c, d)"),
    ] {
        let source = format!(
            "def first(triple: tuple[int, int, int]) -> int:\n    match triple:\n        {pattern} => return a\n"
        );
        let errors = check_str_err(&source, case);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("A tuple pattern has") && error.message.contains("3 elements")),
            "{case}: expected a tuple arity refusal, got {:?}",
            messages(&errors)
        );
    }
    assert_check_ok(&format!(
        "{declarations}def area(shape: Shape) -> float:\n    match shape:\n        Shape.Rect(w, h) => return w * h\n        Shape.Circle(r) => return r\n        Shape.Empty => return 0.0\n\ndef first(triple: tuple[int, int, int]) -> int:\n    match triple:\n        (a, _, _) => return a\n"
    ));
}

/// #1561: every `str` method but `encode` takes its arguments by position, for `str` and `FrozenStr` receivers alike.
#[test]
fn named_arguments_to_str_methods_are_refused_issue1561() {
    for (case, call) in [
        ("replace", "text.replace(old=\"a\", new=\"b\")"),
        ("split", "text.split(separator=\",\")[0]"),
        ("startswith", "str(text.startswith(prefix=\"a\"))"),
    ] {
        let source = format!("def main() -> None:\n    text = \"a,b\"\n    println({call})\n");
        let errors = check_str_err(&source, case);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Unexpected keyword argument")),
            "{case}: expected a keyword refusal, got {:?}",
            messages(&errors)
        );
    }
    let frozen = check_str_err(
        "const NAME: str = \"a,b\"\n\ndef main() -> None:\n    println(NAME.replace(old=\"a\", new=\"b\"))\n",
        "a FrozenStr method with named arguments must be refused",
    );
    assert!(
        frozen
            .iter()
            .any(|error| error.message.contains("Unexpected keyword argument")),
        "expected a keyword refusal on FrozenStr, got {:?}",
        messages(&frozen)
    );
    assert_check_ok(
        "def main() -> None:\n    text = \"a,b\"\n    println(text.replace(\"a\", \"b\"))\n    data = text.encode(encoding=\"utf-8\")\n    println(len(data))\n",
    );
}

/// #1561: a binding is immutable unless declared `mut`, and so is what it holds: a field or element write through a
/// `let` or plain local, or through a parameter not marked `mut`, is refused, compound and tuple forms included.
#[test]
fn writes_through_read_only_bindings_are_refused_issue1561() {
    let declarations = "model Point:\n    x: int\n    y: int\n\n";
    for (case, body) in [
        ("let field", "    let p = Point(x=1, y=2)\n    p.x = 3\n"),
        ("plain field", "    p = Point(x=1, y=2)\n    p.x = 3\n"),
        ("compound field", "    p = Point(x=1, y=2)\n    p.x += 3\n"),
        ("list element", "    items = [1, 2]\n    items[0] = 3\n"),
        ("dict entry", "    let table = {\"a\": 1}\n    table[\"a\"] = 2\n"),
        ("tuple assignment", "    p = Point(x=1, y=2)\n    p.x, p.y = (3, 4)\n"),
        ("nested", "    rows = [[1]]\n    rows[0][0] = 2\n"),
    ] {
        let source = format!("{declarations}def main() -> None:\n{body}");
        let errors = check_str_err(&source, case);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("variable is immutable")),
            "{case}: expected a write-through-immutable refusal, got {:?}",
            messages(&errors)
        );
    }
    let parameter = check_str_err(
        &format!("{declarations}def shift(p: Point) -> None:\n    p.x = 3\n"),
        "a field write through a parameter not marked mut must be refused",
    );
    assert!(
        parameter
            .iter()
            .any(|error| error.message.contains("Cannot mutate 'p'")),
        "expected the parameter write refusal, got {:?}",
        messages(&parameter)
    );
    assert_check_ok(&format!(
        "{declarations}static COUNTS: dict[str, int] = {{\"a\": 1}}\n\ndef shift(mut p: Point) -> None:\n    p.x = 3\n\ndef main() -> None:\n    mut q = Point(x=1, y=2)\n    q.x = 3\n    mut items = [1, 2]\n    items[0] = 3\n    live = COUNTS\n    live[\"a\"] = 2\n    mut points = [Point(x=1, y=2)]\n    for point in points:\n        point.x = 5\n"
    ));
}

/// Build a binding descriptor whose one symbol returns a C text view, `c.ConstPtr[c.c_char]`.
fn text_view_binding() -> CBindingDescriptor {
    CBindingDescriptor {
        span: Span::default(),
        class_name: "Fixture".to_string(),
        header: "fixture.h".to_string(),
        system_library: "fixture".to_string(),
        link_capability: incan_lang::lang::c_abi::LinkCapabilityId::SystemLibrary,
        resources: Vec::new(),
        symbols: vec![CBindingSymbol {
            name: "message".to_string(),
            native: "fixture_message".to_string(),
            parameters: Vec::new(),
            return_type: CBindingType::Pointer {
                mutable: false,
                pointee: Box::new(CBindingType::Scalar(ScalarTypeId::CChar)),
            },
            buffers: Vec::new(),
            outcomes: Vec::new(),
        }],
        enums: Vec::new(),
        structs: Vec::new(),
    }
}

/// Check each statement of `body` (a function body) inside `unsafe:` against the text-view binding.
fn check_text_view_statements(body: &str) -> Vec<CompileError> {
    let source = format!("def main() -> None:\n{body}");
    let program = parse_program(&source, "text view body");
    let mut checker = TypeChecker::new();
    let descriptor = text_view_binding();
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);
    checker.unsafe_depth = 1;
    for declaration in &program.declarations {
        if let Declaration::Function(function) = &declaration.node {
            for statement in &function.body {
                checker.check_statement(statement);
            }
        }
    }
    checker.errors
}

/// #1561: a C text view is bound to a local and copied with `copy_utf8`, or copied at once; storing it, reading its
/// local in any other position, and capturing that local in a closure are refused (RFC 116).
#[test]
fn escaping_c_text_views_are_refused_issue1561() {
    for (case, body) in [
        ("stored in a list", "    views = [Fixture.message()]\n"),
        (
            "local read as a value",
            "    view = Fixture.message()\n    other = view\n",
        ),
        ("local passed on", "    view = Fixture.message()\n    println(view)\n"),
        (
            "local captured",
            "    view = Fixture.message()\n    reader = () => view.copy_utf8(max_bytes=64)\n",
        ),
    ] {
        let errors = check_text_view_statements(body);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("scoped C text view cannot be returned")),
            "{case}: expected a text-view escape refusal, got {:?}",
            messages(&errors)
        );
    }
    for (case, body) in [
        (
            "bound then copied",
            "    view = Fixture.message()\n    text = view.copy_utf8(max_bytes=64)\n",
        ),
        (
            "copied at once",
            "    text = Fixture.message().copy_utf8(max_bytes=64)\n",
        ),
    ] {
        let errors = check_text_view_statements(body);
        assert!(
            !errors.iter().any(|error| error.message.contains("scoped C text view")),
            "{case}: a view copied with copy_utf8 must be accepted, got {:?}",
            messages(&errors)
        );
    }
}

/// #1561: `decimal` and the frozen types have no JSON form, so a JSON declaration holding one, directly or inside a
/// collection, is refused, and so is a route payload holding one.
#[test]
fn json_members_without_a_json_form_are_refused_issue1561() {
    for (case, field) in [
        ("decimal", "amount: decimal[10, 2]"),
        ("FrozenStr", "label: FrozenStr"),
        ("FrozenList", "tags: FrozenList[int]"),
        ("list of decimal", "amounts: list[decimal[10, 2]]"),
    ] {
        let source = format!("from std.serde import json\n\n@derive(json)\nmodel Invoice:\n    {field}\n");
        let errors = check_str_err(&source, case);
        assert!(
            errors.iter().any(|error| error.message.contains("has no JSON form")),
            "{case}: expected a JSON-form refusal, got {:?}",
            messages(&errors)
        );
    }
    assert_check_ok(
        "from std.serde import json\n\n@derive(json)\nmodel Invoice:\n    label: str\n    amounts: list[float]\n",
    );
    let route = check_str_err(
        "import std.async\nfrom std.web import Json, route, GET\n\n\n@route(\"/total\", method=GET)\nasync def total() -> Json[decimal[10, 2]]:\n    return Json(1.50d)\n",
        "a decimal route payload must be refused",
    );
    assert!(
        route.iter().any(|error| error.stable_code() == Some("INCAN-T0112")
            && error.message.contains("has no JSON form")
            && error
                .hints
                .iter()
                .any(|hint| hint.contains("Use a type with a JSON form"))),
        "expected the route payload refusal for decimal, got {:?}",
        messages(&route)
    );
}

/// #1561: an f-string `{value:?}` part renders the value through `Debug`, so a generator, a function and a value whose
/// type implements no `Debug` are refused with `INCAN-T0103` as the `{value}` form refuses them; a model and a list
/// are accepted.
#[test]
fn debug_interpolation_of_a_value_without_debug_is_refused_issue1561() -> Result<(), String> {
    for (case, body) in [
        (
            "generator",
            "def numbers() -> Generator[int]:\n    yield 1\n\ndef main() -> None:\n    gen = numbers()\n    println(f\"{gen:?}\")\n",
        ),
        (
            "function",
            "def helper() -> int:\n    return 1\n\ndef main() -> None:\n    println(f\"{helper:?}\")\n",
        ),
    ] {
        let errors = check_str_err(body, case);
        let refusal = errors
            .iter()
            .find(|error| error.message.contains("with ':?'"))
            .ok_or_else(|| format!("{case}: expected a debug refusal, got {:?}", messages(&errors)))?;
        assert_eq!(refusal.stable_code(), Some("INCAN-T0103"), "{case}");
    }
    assert_check_ok(
        "model Point:\n    x: int\n\ndef main() -> None:\n    point = Point(x=1)\n    items = [1, 2]\n    println(f\"{point:?} {items:?}\")\n",
    );
    Ok(())
}
