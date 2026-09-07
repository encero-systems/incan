//! Formatting for RFC 081 descriptor-gated embedded fragments.
//!
//! RFC 081 gives the formatter exactly two modes and no third fallback: render the fragment from its structured
//! artifact, or preserve the original source verbatim for a DSL that declares itself layout-sensitive. The
//! descriptor owns that choice through `EmbeddedFragmentFormatHint`, and the parser resolves it onto
//! [`EmbeddedFragmentExpr::layout_sensitive`], so everything here reads one already-decided flag.
//!
//! Rendering is idempotent by construction: layout whitespace between structural nodes is dropped and reproduced
//! from the tree's own shape, while content whitespace is collapsed to single spaces. Re-parsing formatted output
//! therefore yields a tree that renders to the same bytes. `RawText` is the deliberate exception to *collapsing* —
//! its whole point is verbatim content, so its text runs are never collapsed — but not to idempotency: like the
//! layout-preserving mode, it drops the trailing newline the enclosing block re-supplies (see
//! [`without_trailing_block_newlines`]), which is what stops a preserved fragment from growing a blank line on
//! every formatting pass.

use crate::frontend::ast::{
    EmbeddedAttr, EmbeddedDeclaration, EmbeddedFragmentExpr, EmbeddedNode, EmbeddedStyleRule, EmbeddedTypeShape,
    EmbeddedValue, Spanned,
};
use incan_vocab::EmbeddedFragmentSubmode;

use super::Formatter;

/// Whether a node is pure layout between structural siblings rather than content.
///
/// The parser preserves the whitespace that separated one element or rule from the next. That spacing belongs to
/// the source's layout, not to the fragment's meaning, so a structural render drops it and re-derives it. Keeping
/// it would make formatting accumulate indentation on every pass.
fn is_layout_whitespace(node: &EmbeddedNode) -> bool {
    matches!(node, EmbeddedNode::Text(text) if text.trim().is_empty())
}

/// Collapse a content text run to single spaces.
///
/// Applied to markup text only. Collapsing is a fixed point: running it over already-collapsed text returns the
/// same string, which is what keeps the formatter idempotent across passes.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip the trailing line breaks that a fragment's preserved text carries but the enclosing block re-supplies.
///
/// A fragment's body runs from the indent that opened it to the dedent that closes it, so its preserved text ends
/// with the newline that terminated its last line. The statement writer emits that terminator itself. The two
/// paths that write preserved text verbatim — a layout-sensitive descriptor, and the `RawText` submode's content
/// runs — would otherwise emit it twice, and every `incan fmt` pass would add another blank line to the file.
/// Trailing newlines immediately before a dedent are block layout rather than fragment content, so dropping them
/// keeps both modes idempotent without touching anything the DSL actually owns.
fn without_trailing_block_newlines(text: &str) -> &str {
    text.trim_end_matches('\n')
}

/// Re-escape a template string's literal text so re-parsing the rendered fragment yields the same text back.
///
/// `parse_template_string` resolves `` \` ``, `\$`, `\\` and `\n` while parsing, so the `Text` nodes it produces
/// hold the *resolved* characters. Writing them back raw would change what the fragment means — a literal
/// backtick would close the string early — so each one is escaped again on the way out. Escaping every `$` rather
/// than only the ones before `{` is deliberate: it costs nothing, and it removes the case where appending a hole
/// next to a literal `$` would silently turn it into an interpolation.
fn escape_template_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '`' => escaped.push_str("\\`"),
            '$' => escaped.push_str("\\$"),
            '\n' => escaped.push_str("\\n"),
            other => escaped.push(other),
        }
    }
    escaped
}

impl Formatter {
    /// Format one embedded fragment, honoring the claiming descriptor's layout-sensitivity declaration.
    pub(super) fn format_embedded_fragment(&mut self, fragment: &EmbeddedFragmentExpr) {
        if fragment.layout_sensitive {
            self.writer
                .write(without_trailing_block_newlines(&fragment.source_text));
            return;
        }
        self.format_embedded_nodes(&fragment.nodes, fragment.submode);
    }

    /// Render a fragment's top-level nodes for its submode.
    fn format_embedded_nodes(&mut self, nodes: &[Spanned<EmbeddedNode>], submode: EmbeddedFragmentSubmode) {
        if matches!(submode, EmbeddedFragmentSubmode::RawText) {
            let last = nodes.len().saturating_sub(1);
            for (index, node) in nodes.iter().enumerate() {
                self.format_embedded_raw_node(&node.node, index == last);
            }
            return;
        }
        if matches!(submode, EmbeddedFragmentSubmode::RegexTemplate) {
            self.format_embedded_regex_template(nodes);
            return;
        }

        let mut first = true;
        for node in nodes {
            if is_layout_whitespace(&node.node) {
                continue;
            }
            if !first {
                self.writer.newline();
            }
            first = false;
            self.format_embedded_node(&node.node, submode);
        }
    }

    /// Render a `RawText` node, preserving its content exactly.
    ///
    /// A raw-text submode exists to keep content untouched, so collapsing or re-indenting it here would change the
    /// program's meaning rather than its layout. `is_last` marks the final node of the fragment, whose trailing
    /// newline belongs to the enclosing block rather than to the content — see
    /// [`without_trailing_block_newlines`].
    fn format_embedded_raw_node(&mut self, node: &EmbeddedNode, is_last: bool) {
        match node {
            EmbeddedNode::Text(text) if is_last => self.writer.write(without_trailing_block_newlines(text)),
            EmbeddedNode::Text(text) => self.writer.write(text),
            EmbeddedNode::Hole(expr) => self.format_embedded_hole(expr),
            other => self.format_embedded_node(other, EmbeddedFragmentSubmode::RawText),
        }
    }

    /// Render a `RegexTemplate` fragment as one of the exactly two forms that submode accepts.
    ///
    /// A regex literal keeps its own node, but a template string's backticks and `${...}` delimiters are consumed
    /// during parsing and never stored, so the node tree alone is `Text`/`Hole` runs indistinguishable from raw
    /// content. Rendering those the way every other submode renders its nodes would emit `hello {name}!` for
    /// `` `hello ${name}!` `` — not merely different layout, but a fragment this submode rejects outright, so
    /// `incan fmt` would rewrite a valid file into one that no longer parses. Reconstructing the delimiters is
    /// what makes the structural mode a faithful round trip here.
    fn format_embedded_regex_template(&mut self, nodes: &[Spanned<EmbeddedNode>]) {
        if let Some(only) = nodes.first()
            && nodes.len() == 1
            && matches!(only.node, EmbeddedNode::Regex { .. })
        {
            self.format_embedded_node(&only.node, EmbeddedFragmentSubmode::RegexTemplate);
            return;
        }

        self.writer.write("`");
        for node in nodes {
            match &node.node {
                EmbeddedNode::Text(text) => self.writer.write(&escape_template_text(text)),
                EmbeddedNode::Hole(expr) => {
                    self.writer.write("$");
                    self.format_embedded_hole(expr);
                }
                other => self.format_embedded_node(other, EmbeddedFragmentSubmode::RegexTemplate),
            }
        }
        self.writer.write("`");
    }

    /// Render one structural node in block position.
    fn format_embedded_node(&mut self, node: &EmbeddedNode, submode: EmbeddedFragmentSubmode) {
        match node {
            EmbeddedNode::Element(element) => self.format_embedded_element(element, submode),
            EmbeddedNode::StyleRule(rule) => self.format_embedded_style_rule(rule, submode),
            EmbeddedNode::Declaration(declaration) => {
                self.format_embedded_declaration(declaration, submode);
                self.writer.write(";");
            }
            EmbeddedNode::Comment(text) => self.format_embedded_comment(text, submode),
            EmbeddedNode::Text(text) => self.writer.write(&collapse_whitespace(text)),
            EmbeddedNode::EntityRef(name) => {
                self.writer.write("&");
                self.writer.write(name);
                self.writer.write(";");
            }
            EmbeddedNode::Hole(expr) => self.format_embedded_hole(expr),
            EmbeddedNode::Value(value) => self.format_embedded_value(value),
            EmbeddedNode::Regex { pattern, flags } => {
                self.writer.write("/");
                self.writer.write(pattern);
                self.writer.write("/");
                self.writer.write(flags);
            }
            EmbeddedNode::TypeShape(shape) => self.format_embedded_type_shape(shape),
        }
    }

    /// Render an expression hole through ordinary Incan expression formatting.
    ///
    /// A hole is genuine Incan, so it is formatted by the same code that formats the expression anywhere else
    /// rather than reproduced from source text.
    fn format_embedded_hole(&mut self, expr: &Spanned<crate::frontend::ast::Expr>) {
        self.writer.write("{");
        self.format_expr(&expr.node);
        self.writer.write("}");
    }

    /// Render a comment using the owning submode's delimiters.
    fn format_embedded_comment(&mut self, text: &str, submode: EmbeddedFragmentSubmode) {
        let body = collapse_whitespace(text);
        match submode {
            EmbeddedFragmentSubmode::Style | EmbeddedFragmentSubmode::SelectorDeclarationValue => {
                self.writer.write("/* ");
                self.writer.write(&body);
                self.writer.write(" */");
            }
            _ => {
                self.writer.write("<!-- ");
                self.writer.write(&body);
                self.writer.write(" -->");
            }
        }
    }

    /// Render a markup element, its attributes, and its children.
    ///
    /// Children stay on one line unless one of them is itself a block shape (a nested element or a comment); that
    /// keeps `<h1>Hello {name}</h1>` intact rather than exploding text and holes onto separate lines.
    fn format_embedded_element(
        &mut self,
        element: &crate::frontend::ast::EmbeddedElement,
        submode: EmbeddedFragmentSubmode,
    ) {
        self.writer.write("<");
        self.writer.write(&element.name);
        for attr in &element.attrs {
            self.writer.write(" ");
            self.format_embedded_attr(attr, submode);
        }
        if element.self_closing {
            self.writer.write(" />");
            return;
        }
        self.writer.write(">");

        let has_block_child = element
            .children
            .iter()
            .any(|child| matches!(child.node, EmbeddedNode::Element(_) | EmbeddedNode::Comment(_)));
        if has_block_child {
            self.writer.indent();
            for child in &element.children {
                if is_layout_whitespace(&child.node) {
                    continue;
                }
                self.writer.newline();
                self.format_embedded_node(&child.node, submode);
            }
            self.writer.dedent();
            self.writer.newline();
        } else {
            self.format_embedded_inline_children(&element.children, submode);
        }

        self.writer.write("</");
        self.writer.write(&element.name);
        self.writer.write(">");
    }

    /// Render an element's children on one line, trimming the whitespace that hugged the tags.
    fn format_embedded_inline_children(
        &mut self,
        children: &[Spanned<EmbeddedNode>],
        submode: EmbeddedFragmentSubmode,
    ) {
        let significant: Vec<&Spanned<EmbeddedNode>> = children
            .iter()
            .filter(|child| !is_layout_whitespace(&child.node))
            .collect();
        let last = significant.len().saturating_sub(1);
        for (index, child) in significant.iter().enumerate() {
            match &child.node {
                EmbeddedNode::Text(text) => {
                    let mut rendered = collapse_whitespace(text);
                    // Whitespace that only separated the text from its enclosing tags is layout; whitespace between
                    // this run and a sibling hole is content, so it survives as the single space collapsing left.
                    if index == 0 && text.starts_with(char::is_whitespace) {
                        rendered = rendered.trim_start().to_string();
                    }
                    if index == last && text.ends_with(char::is_whitespace) {
                        rendered = rendered.trim_end().to_string();
                    }
                    if index != 0 && text.starts_with(char::is_whitespace) {
                        self.writer.write(" ");
                    }
                    self.writer.write(&rendered);
                    if index != last && text.ends_with(char::is_whitespace) {
                        self.writer.write(" ");
                    }
                }
                other => self.format_embedded_node(other, submode),
            }
        }
    }

    /// Render one markup attribute, quoting a literal value and bracing a hole.
    fn format_embedded_attr(&mut self, attr: &EmbeddedAttr, submode: EmbeddedFragmentSubmode) {
        self.writer.write(&attr.name);
        let Some(value) = &attr.value else {
            return;
        };
        self.writer.write("=");
        match &value.node {
            EmbeddedNode::Text(text) => {
                self.writer.write("\"");
                self.writer.write(text);
                self.writer.write("\"");
            }
            other => self.format_embedded_node(other, submode),
        }
    }

    /// Render a style rule as a selector list and an indented declaration block.
    fn format_embedded_style_rule(&mut self, rule: &EmbeddedStyleRule, submode: EmbeddedFragmentSubmode) {
        let selectors: Vec<&Spanned<EmbeddedNode>> = rule
            .selectors
            .iter()
            .filter(|selector| !is_layout_whitespace(&selector.node))
            .collect();
        for (index, selector) in selectors.iter().enumerate() {
            if index != 0 {
                self.writer.write(", ");
            }
            self.format_embedded_node(&selector.node, submode);
        }
        self.writer.write(" {");
        self.writer.indent();
        for declaration in &rule.declarations {
            if is_layout_whitespace(&declaration.node) {
                continue;
            }
            self.writer.newline();
            self.format_embedded_node(&declaration.node, submode);
        }
        self.writer.dedent();
        self.writer.newline();
        self.writer.write("}");
    }

    /// Render one `property: value` declaration without its trailing semicolon.
    ///
    /// The semicolon is the caller's, because a declaration inside a rule block terminates while the bare
    /// declaration-value submode does not.
    fn format_embedded_declaration(&mut self, declaration: &EmbeddedDeclaration, submode: EmbeddedFragmentSubmode) {
        self.writer.write(&declaration.property);
        self.writer.write(":");
        for value in &declaration.value {
            if is_layout_whitespace(&value.node) {
                continue;
            }
            self.writer.write(" ");
            self.format_embedded_node(&value.node, submode);
        }
    }

    /// Render one declaration-value or selector-position literal.
    fn format_embedded_value(&mut self, value: &EmbeddedValue) {
        match value {
            EmbeddedValue::Dimension { number, unit } => {
                self.writer.write(number);
                self.writer.write(unit);
            }
            EmbeddedValue::Color(text)
            | EmbeddedValue::CustomPropertyRef(text)
            | EmbeddedValue::Ident(text)
            | EmbeddedValue::Number(text)
            | EmbeddedValue::Selector(text) => self.writer.write(text),
            EmbeddedValue::StringLit(text) => {
                self.writer.write("\"");
                self.writer.write(text);
                self.writer.write("\"");
            }
        }
    }

    /// Render a type-shaped grammar node.
    fn format_embedded_type_shape(&mut self, shape: &EmbeddedTypeShape) {
        match shape {
            EmbeddedTypeShape::Name(segments) => self.writer.write(&segments.join(".")),
            EmbeddedTypeShape::Generic(base, args) => {
                self.format_embedded_type_shape(base);
                self.writer.write("<");
                for (index, arg) in args.iter().enumerate() {
                    if index != 0 {
                        self.writer.write(", ");
                    }
                    self.format_embedded_type_shape(arg);
                }
                self.writer.write(">");
            }
            EmbeddedTypeShape::Nullable(inner) => {
                self.format_embedded_type_shape(inner);
                self.writer.write("?");
            }
            EmbeddedTypeShape::Array(inner) => {
                self.format_embedded_type_shape(inner);
                self.writer.write("[]");
            }
            EmbeddedTypeShape::Union(members) => {
                for (index, member) in members.iter().enumerate() {
                    if index != 0 {
                        self.writer.write(" | ");
                    }
                    self.format_embedded_type_shape(member);
                }
            }
        }
    }
}
