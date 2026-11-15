//! Embedded fragment painting, character-aligned runs, and comment ranges.

use super::*;

// ============================================================================
// LAYER 3 — EMBEDDED FRAGMENTS
// ============================================================================

/// The category DSL-owned bytes take, chosen per submode.
///
/// Ordinary Incan never renders as `macro`, so a markup or style fragment reads at a glance as a different surface —
/// which is exactly the ownership visibility RFC 081 asks tooling to provide. Raw text, regex and type-shaped
/// submodes have honest standard equivalents and use those instead.
pub(super) fn submode_category(submode: incan_vocab::EmbeddedFragmentSubmode) -> Category {
    use incan_vocab::EmbeddedFragmentSubmode;
    match submode {
        EmbeddedFragmentSubmode::Markup
        | EmbeddedFragmentSubmode::Style
        | EmbeddedFragmentSubmode::SelectorDeclarationValue => Category::Macro,
        EmbeddedFragmentSubmode::RawText => Category::String,
        EmbeddedFragmentSubmode::RegexTemplate => Category::Regexp,
        EmbeddedFragmentSubmode::TypePosition => Category::Type,
        // The submode list is `#[non_exhaustive]`: a submode added later is DSL-owned by definition, so treat it as
        // such rather than letting its bytes fall through to ordinary-Incan classification.
        _ => Category::Macro,
    }
}

/// Replace the token layer's answer over every embedded fragment in the program.
///
/// This routes through [`any_expr_in_program`], the compiler's own expression walk, rather than adding a seventh
/// private copy of "which expressions does a program contain" (#1022 deleted six such duplicates). The painting
/// happens inside the walk because that walk lends each expression only for the duration of the call — which is
/// also why nothing here clones a fragment to carry it out.
pub(super) fn apply_fragments(program: &Program, source: &str, map: &mut [Option<Paint>]) {
    any_expr_in_program(program, |expr| {
        if let Expr::Embedded(fragment) = expr {
            apply_fragment(fragment, source, map);
        }
        false
    });
}

/// Clear one fragment's extent from the token layer, then paint its own classification over it.
///
/// Hole bytes are deliberately left as the token layer classified them: a hole is ordinary Incan, typechecked and
/// lowered exactly as the same expression in ordinary position, so re-deriving it here would be a second opinion
/// about what the fragment owns.
///
/// One boundary is worth stating, because it is easy to read as a disagreement with
/// [`EmbeddedFragmentExpr::ownership_at`] and is not. That method answers for a *cursor position*, where the offset
/// just past a name still resolves to it — the right convention for hover. This paints *bytes*, and the byte at a
/// hole's end is the hole's closing delimiter, which is DSL-owned punctuation rather than part of the expression.
/// The two answers coincide everywhere else.
pub(super) fn apply_fragment(
    fragment: &incan_syntax::ast::EmbeddedFragmentExpr,
    source: &str,
    map: &mut [Option<Paint>],
) {
    let Some((extent, ranges)) = fragment_ranges(fragment, source) else {
        return;
    };
    let holes: Vec<_> = fragment.holes().iter().map(|hole| hole.span).collect();
    let limit = map.len();
    for (offset, slot) in map
        .iter_mut()
        .enumerate()
        .take(extent.end.min(limit))
        .skip(extent.start.min(limit))
    {
        let in_hole = holes.iter().any(|span| span.start <= offset && offset < span.end);
        if !in_hole {
            *slot = None;
        }
    }
    paint(map, &ranges);
}

/// Classify one fragment's bytes, returning its extent alongside the ranges covering the DSL-owned part.
///
/// Every byte of the extent is decided here except the holes, which are left uncovered so the token-stream layer's
/// ordinary-Incan classification shows through. That inversion is deliberate: a hole is not embedded syntax at all,
/// and the compiler already typechecks it exactly as it would the same expression in ordinary position.
pub(super) fn fragment_ranges(
    fragment: &incan_syntax::ast::EmbeddedFragmentExpr,
    source: &str,
) -> Option<(incan_syntax::ast::Span, Vec<ClassifiedRange>)> {
    let extent = fragment.node_extent()?;
    let start = extent.start.min(source.len());
    let end = extent.end.min(source.len());
    if end <= start {
        return None;
    }

    // A byte map is the simplest way to let node categories, nested children and holes all overwrite one another in
    // tree order without an interval-arithmetic pass. Fragments are editor-sized, so the cost is irrelevant.
    let default = submode_category(fragment.submode);
    let mut bytes = vec![Some(default); end - start];
    paint_nodes(&fragment.nodes, start, &mut bytes);
    for hole in fragment.holes() {
        for offset in hole.span.start.max(start)..hole.span.end.min(end) {
            bytes[offset - start] = None;
        }
    }

    Some((extent, run_length_ranges(&bytes, start, source)))
}

/// Paint node-specific categories over a fragment's byte map, in tree order so children refine their parents.
pub(super) fn paint_nodes(nodes: &[Spanned<EmbeddedNode>], origin: usize, bytes: &mut [Option<Category>]) {
    for node in nodes {
        let category = match &node.node {
            EmbeddedNode::Comment(_) => Some(Category::Comment),
            EmbeddedNode::Regex { .. } => Some(Category::Regexp),
            EmbeddedNode::Text(_) => Some(Category::String),
            EmbeddedNode::EntityRef(_) => Some(Category::Macro),
            _ => None,
        };
        if let Some(category) = category {
            for offset in node.span.start.max(origin)..node.span.end.min(origin + bytes.len()) {
                bytes[offset - origin] = Some(category);
            }
        }
        // Recurse so a child's own category wins over the container's, and so holes nested inside containers are
        // reached. The hole pass runs afterwards and overwrites whatever was painted here.
        match &node.node {
            EmbeddedNode::Element(element) => {
                for attr in &element.attrs {
                    if let Some(value) = &attr.value {
                        paint_nodes(std::slice::from_ref(value), origin, bytes);
                    }
                }
                paint_nodes(&element.children, origin, bytes);
            }
            EmbeddedNode::StyleRule(rule) => {
                paint_nodes(&rule.selectors, origin, bytes);
                paint_nodes(&rule.declarations, origin, bytes);
            }
            EmbeddedNode::Declaration(declaration) => {
                paint_nodes(&declaration.value, origin, bytes);
            }
            _ => {}
        }
    }
}

/// Collapse a byte map into the fewest contiguous ranges, skipping bytes left to another layer.
///
/// Cuts land only where the byte map changes, and every boundary in it comes from a node or hole span, so each
/// emitted range starts and ends on a character boundary.
pub(super) fn run_length_ranges(bytes: &[Option<Category>], origin: usize, source: &str) -> Vec<ClassifiedRange> {
    let mut ranges = Vec::new();
    let mut run: Option<(usize, Category)> = None;
    for (index, byte) in bytes.iter().enumerate() {
        let offset = origin + index;
        match (run, byte) {
            (Some((start, category)), Some(current)) if category == *current => {
                let _ = start;
            }
            (Some((start, category)), _) => {
                push_char_aligned(&mut ranges, start, offset, category, source);
                run = byte.map(|category| (offset, category));
            }
            (None, Some(category)) => run = Some((offset, *category)),
            (None, None) => {}
        }
    }
    if let Some((start, category)) = run {
        push_char_aligned(&mut ranges, start, origin + bytes.len(), category, source);
    }
    ranges
}

/// Record a range, snapping both ends onto character boundaries so a multi-byte character is never split.
pub(super) fn push_char_aligned(
    ranges: &mut Vec<ClassifiedRange>,
    start: usize,
    end: usize,
    category: Category,
    source: &str,
) {
    let mut start = start;
    let mut end = end;
    while start < source.len() && !source.is_char_boundary(start) {
        start += 1;
    }
    while end > start && end <= source.len() && !source.is_char_boundary(end) {
        end -= 1;
    }
    if end > start {
        ranges.push(ClassifiedRange::plain(start, end, category));
    }
}

// ============================================================================
// COMMENTS
// ============================================================================

/// Recover comment ranges as the `#`-to-end-of-line runs no other layer claimed.
///
/// The lexer discards comments rather than emitting them as tokens, and the formatter's comment model is anchored to
/// declarations rather than to spans, so neither can be reused here. Rather than re-implement string and fragment
/// scanning — the two places a `#` is not a comment — this asks what the other layers already covered: a `#` inside
/// a string literal or a claimed fragment is inside a classified range, and only an unclaimed one starts a comment.
pub(super) fn comment_ranges(source: &str, claimed: &[Option<Paint>]) -> Vec<ClassifiedRange> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(hash) = source[offset..].find('#') {
        let start = offset + hash;
        offset = start + 1;
        if claimed.get(start).is_some_and(Option::is_some) {
            continue;
        }
        let end = source[start..]
            .find('\n')
            .map_or(source.len(), |newline| start + newline);
        ranges.push(ClassifiedRange::plain(start, end, Category::Comment));
        offset = end;
    }
    ranges
}
