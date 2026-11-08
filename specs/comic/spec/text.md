# COMIC text

Status: syntax direction and boundary, not a frozen grammar. The corpus's JSON notation is not this text format.

COMIC describes typed values. It has no imports, statements, callable expressions or evaluation. Names label types, records and references; parentheses group positional data. Seeing a function description does not load or run that function.

## Intended readable forms

These examples are proposals for a later grammar corpus, not parser goldens:

~~~text
example.Customer { name = "Ana", parcels = [10, 20] }
example.Status.Ready
example.Shape.Circle(1.0)
~~~

Named fields use braces, positional variant payloads use parentheses, and sequences keep order. Optional values can print bare where the admitted type makes the wrapping unambiguous. Nested absence must remain expressible, such as Some(None). A typed table may group consecutive values of one description without changing their identities or sequence order.

Compiler declarations are ordinary records under a language profile. Incan.Function, Rust.Struct and similar descriptions do not belong to the neutral grammar. Language-specific paths and overload spelling belong to that profile. A named document entry is a data binding, not a new declaration in the executing program.

## Grammar work before text conformance

Define the document/profile envelope, admitted descriptions, escaped names, references, integer and decimal grammar, shortest float rendering, signed zero, NaN, string/control escapes, bytes, nested optionals, empty maps versus sets, named and positional variants, recursive descriptions and table grouping. Define comment/whitespace acceptance independently from canonical printing. In particular, presentation order must not replace member identity order in value hashing.

The printer must have exact rules for whitespace, field presentation and grouping; it must not depend on terminal width, locale, host map iteration or a language formatter. Readers may accept noncanonical whitespace and comments if specified; writers produce one canonical form. Publish input, expected typed value and exact canonical text together before claiming parser or printer conformance.

Redacted diagnostic text is clearly labeled and cannot be reconstructed as a complete value. A secret-typed value prints as `<secret>` in canonical text, and reading it yields a withheld value. Every other redaction marker is diagnostic and is refused on reading. Reading a text reference does not resolve files, packages, network resources or store entries.
