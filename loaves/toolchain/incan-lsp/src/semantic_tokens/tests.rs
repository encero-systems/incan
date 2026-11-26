use super::*;
use incan_syntax::parser;

type TestResult = Result<(), String>;

/// Parse a source into a program, as the language server does for a document that compiles.
fn parse(source: &str) -> Result<Program, String> {
    let (tokens, _lex_errors) = lexer::lex_tolerant(source);
    parser::parse_with_source(&tokens, None, None, None, source).map_err(|errors| format!("parse errors: {errors:?}"))
}

/// Return the category covering the first occurrence of `needle`, or an error naming what was found instead.
fn category_of(source: &str, ranges: &[ClassifiedRange], needle: &str) -> Result<Category, String> {
    let offset = source
        .find(needle)
        .ok_or_else(|| format!("fixture does not contain {needle:?}"))?;
    ranges
        .iter()
        .find(|range| range.start <= offset && offset < range.end)
        .map(|range| range.category)
        .ok_or_else(|| format!("{needle:?} at byte {offset} is unclassified"))
}

/// Classify a source that parses, exercising every layer.
fn classify(source: &str) -> Result<Vec<ClassifiedRange>, String> {
    let program = parse(source)?;
    Ok(classified_ranges(source, Some(&program)))
}

/// Pin each category's discriminant to its position in the published legend.
#[test]
fn legend_indices_match_the_category_discriminants() {
    // The client resolves `tokenType` by indexing into the published legend, so a category whose discriminant
    // drifts from its position in TOKEN_TYPES silently recolors every document rather than failing loudly.
    assert_eq!(TOKEN_TYPES.len(), Category::Macro.index() as usize + 1);
    assert_eq!(
        TOKEN_TYPES[Category::Keyword.index() as usize],
        SemanticTokenType::KEYWORD
    );
    assert_eq!(TOKEN_TYPES[Category::Type.index() as usize], SemanticTokenType::TYPE);
    assert_eq!(TOKEN_TYPES[Category::Macro.index() as usize], SemanticTokenType::MACRO);
    assert_eq!(TOKEN_MODIFIERS.len(), Modifier::DefaultLibrary as usize + 1);
    assert_eq!(
        TOKEN_MODIFIERS[Modifier::Declaration as usize],
        SemanticTokenModifier::DECLARATION
    );
}

/// A declaration's name takes the kind of the thing it declares, not a generic identifier color.
#[test]
fn a_declaration_name_is_the_thing_it_declares() -> TestResult {
    // The problem statement's first complaint: a regex grammar cannot tell a declaration name from a local.
    let source = "def compute(value: int) -> int:\n    return value\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "compute")?, Category::Function);
    assert_eq!(category_of(source, &ranges, "def")?, Category::Keyword);
    Ok(())
}

/// Type positions are decided by where the parser put them, never by how the name is capitalized.
#[test]
fn type_positions_come_from_the_ast_rather_than_from_spelling() -> TestResult {
    // `str` is lowercase and `Total` is PascalCase, so any spelling heuristic gets both of these backwards.
    // The AST places one in a type slot and the other in a local binding, and that is what decides.
    let source = "def render(label: str) -> None:\n    Total = label\n    return None\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "str")?, Category::Type);
    assert_eq!(category_of(source, &ranges, "Total")?, Category::Variable);
    assert_eq!(category_of(source, &ranges, "label: ")?, Category::Parameter);
    Ok(())
}

/// A model, a trait and an enum each highlight as their own kind rather than as one shared category.
#[test]
fn nominal_declarations_take_their_own_kinds() -> TestResult {
    let source = "model User:\n    id: int\n\ntrait Loggable:\n    pass\n\nenum Mode:\n    Fast\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "User")?, Category::Struct);
    assert_eq!(category_of(source, &ranges, "Loggable")?, Category::Interface);
    assert_eq!(category_of(source, &ranges, "Mode")?, Category::Enum);
    Ok(())
}

/// Member access distinguishes a called method from a property that is only read.
#[test]
fn member_access_separates_a_called_method_from_a_read_property() -> TestResult {
    let source = "def use(value: str) -> None:\n    other = value.lower()\n    size = value.length\n    return None\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "lower")?, Category::Method);
    assert_eq!(category_of(source, &ranges, "length")?, Category::Property);
    Ok(())
}

/// Every segment of a dotted path takes the meaning of whatever introduced the path.
#[test]
fn a_dotted_path_takes_the_meaning_of_what_introduced_it() -> TestResult {
    // Only the token to the immediate left is visible to the member-access rule, so every segment after the
    // first would otherwise read as a field access on a local named `std`.
    let source = "from std.runtime import host\n\ndef main() -> None:\n    return None\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "std")?, Category::Namespace);
    assert_eq!(category_of(source, &ranges, "runtime")?, Category::Namespace);
    Ok(())
}

/// Comment recovery claims only bytes no other layer owns, so a `#` inside a literal stays string content.
#[test]
fn a_hash_inside_a_string_does_not_start_a_comment() -> TestResult {
    // Comment recovery asks which bytes no other layer claimed rather than re-implementing string scanning,
    // so a `#` inside a literal has to stay part of the string.
    let source = "def main() -> None:\n    tag = \"#not-a-comment\"  # a real comment\n    return None\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "#not-a-comment")?, Category::String);
    assert_eq!(category_of(source, &ranges, "# a real comment")?, Category::Comment);
    Ok(())
}

/// An f-string's interpolated expressions are classified as code, not as string content.
#[test]
fn an_fstring_hole_is_classified_as_ordinary_incan() -> TestResult {
    // An f-string is how most Incan code produces output. Treating one as a flat string blob would leave the
    // names inside it — the part a reader most needs to follow — uncolored.
    let source = "def greet(name: str) -> None:\n    println(f\"hello {name}\")\n    return None\n";
    let ranges = classify(source)?;
    assert_eq!(category_of(source, &ranges, "hello ")?, Category::String);
    assert_eq!(category_of(source, &ranges, "name}")?, Category::Variable);
    Ok(())
}

/// A document mid-edit still receives highlighting from the token stream alone.
#[test]
fn a_document_that_does_not_parse_is_still_classified() -> TestResult {
    // The ordinary state of a document being edited. Without an AST the type and fragment layers are gone, but
    // losing every color on each keystroke would be worse than never having had semantic highlighting.
    let source = "def compute(value: int) -> int:\n    return value +\n";
    let ranges = classified_ranges(source, None);
    assert_eq!(category_of(source, &ranges, "def")?, Category::Keyword);
    assert_eq!(category_of(source, &ranges, "compute")?, Category::Function);
    assert_eq!(category_of(source, &ranges, "return")?, Category::Keyword);
    Ok(())
}

/// Columns are reported in UTF-16 code units, as the protocol's default position encoding requires.
#[test]
fn columns_are_counted_in_utf16_units_not_bytes() -> TestResult {
    // The balloon is four bytes but two UTF-16 code units, so `suffix` sits at byte column 21 and UTF-16
    // column 19. A byte-counted column places it two characters to the right of where the reader sees it —
    // the class of bug that makes highlighting look almost, but not quite, aligned.
    let source = "def main(suffix: str) -> None:\n    total = \"\u{1F388}\" + suffix\n    return None\n";
    let ranges = classify(source)?;
    let offset = source
        .rfind("suffix")
        .ok_or_else(|| "fixture must use `suffix` in the interpolating line".to_string())?;
    let range = ranges
        .iter()
        .find(|range| range.start <= offset && offset < range.end)
        .ok_or_else(|| "the trailing `suffix` must be classified".to_string())?;
    let byte_column = offset - source[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    assert_eq!(byte_column, 21, "fixture assumption: `suffix` starts at byte column 21");
    let pieces = line_pieces_indexed(source, &LineIndex::new(source), range.start, range.end);
    assert_eq!(
        pieces,
        vec![(1, 19, 6)],
        "`suffix` must be reported at UTF-16 column 19"
    );
    Ok(())
}

/// Decoding the delta stream reproduces every classified position exactly.
#[test]
fn each_token_is_encoded_relative_to_the_one_before_it() -> TestResult {
    // Delta encoding is the protocol's wire format, so an absolute column leaking into the stream would shift
    // every token after it. Decoding the deltas back has to reproduce the classified ranges exactly.
    let source = "def compute(value: int) -> int:\n    doubled = value\n    return doubled\n";
    let ranges = classify(source)?;
    let encoded = encode(source, &ranges);
    let index = LineIndex::new(source);
    let mut expected = Vec::new();
    for range in &ranges {
        expected.extend(line_pieces_indexed(source, &index, range.start, range.end));
    }

    let (mut line, mut start) = (0u32, 0u32);
    let mut decoded = Vec::new();
    for token in &encoded {
        line += token.delta_line;
        start = if token.delta_line == 0 {
            start + token.delta_start
        } else {
            token.delta_start
        };
        decoded.push((line, start, token.length));
    }
    assert_eq!(
        decoded, expected,
        "decoding the delta stream must reproduce every classified position"
    );
    Ok(())
}

/// A range covering a newline becomes one token per line, since the protocol forbids a token that spans lines.
#[test]
fn a_multi_line_range_is_split_into_one_token_per_line() -> TestResult {
    // The protocol forbids a token that spans lines, so a triple-quoted string has to be emitted per line.
    let source = "def main() -> None:\n    text = \"\"\"first\nsecond\"\"\"\n    return None\n";
    let program = parse(source)?;
    let ranges = classified_ranges(source, Some(&program));
    let multi_line = ranges
        .iter()
        .find(|range| source[range.start..range.end].contains('\n'))
        .ok_or_else(|| "fixture must produce a range covering a newline".to_string())?;
    let pieces = line_pieces_indexed(source, &LineIndex::new(source), multi_line.start, multi_line.end);
    assert!(
        pieces.len() > 1,
        "a range covering a newline must split into several tokens"
    );
    let lines: Vec<u32> = pieces.iter().map(|(line, _, _)| *line).collect();
    assert!(
        lines.windows(2).all(|pair| pair[0] < pair[1]),
        "pieces must be in line order: {lines:?}"
    );
    Ok(())
}

/// Composed ranges are disjoint and ordered, which delta encoding depends on.
#[test]
fn classified_ranges_never_overlap_and_stay_in_order() -> TestResult {
    // Composition happens over a byte map precisely so this holds. A client given overlapping tokens renders
    // undefined results, and delta encoding an out-of-order range corrupts every token after it.
    let source = "def greet(name: str) -> None:\n    println(f\"hi {name}\")  # wave\n    return None\n";
    let ranges = classify(source)?;
    for pair in ranges.windows(2) {
        assert!(
            pair[0].end <= pair[1].start,
            "ranges must be disjoint and ordered: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    Ok(())
}
