from __future__ import annotations

import re
from pathlib import Path
from typing import Iterable, Sequence

from pygments.lexers import _mapping
from pygments.lexers.python import PythonLexer
from pygments.token import Keyword, Name, Operator, Token


def _load_keywords_from_registry() -> list[str]:
    """Load canonical keywords from the Rust registry file."""
    repo_root = Path(__file__).resolve().parents[2]
    registry_path = repo_root / "loaves" / "kernel" / "incan_lang" / "src" / "lang" / "keywords.rs"
    if not registry_path.exists():
        return []

    text = registry_path.read_text(encoding="utf-8", errors="replace")
    pattern = re.compile(
        r'info(?:_with_aliases)?\(\s*KeywordId::[A-Za-z_]+,\s*"([^"]+)"',
        re.DOTALL,
    )
    return sorted({match.group(1) for match in pattern.finditer(text)})


_INFO_WITH_ALIASES = re.compile(
    r'info\([^,]+,\s*"([^"]+)"\s*,\s*&\[(.*?)\]',
    re.DOTALL,
)
_INFO_CANONICAL = re.compile(r'info\([^,]+,\s*"([^"]+)"', re.DOTALL)


def _extract_lang_items(text: str) -> tuple[set[str], set[str]]:
    canonicals: set[str] = set()
    aliases: set[str] = set()

    for match in _INFO_WITH_ALIASES.finditer(text):
        canonicals.add(match.group(1))
        alias_blob = match.group(2)
        aliases.update(re.findall(r'"([^"]+)"', alias_blob))

    for match in _INFO_CANONICAL.finditer(text):
        canonicals.add(match.group(1))

    return canonicals, aliases


def _load_lang_items(paths: Sequence[Path]) -> tuple[set[str], set[str]]:
    canonicals: set[str] = set()
    aliases: set[str] = set()
    for path in paths:
        if not path.exists():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        c, a = _extract_lang_items(text)
        canonicals.update(c)
        aliases.update(a)
    return canonicals, aliases


def _load_stdlib_functions() -> set[str]:
    repo_root = Path(__file__).resolve().parents[3]
    base = repo_root / "loaves" / "kernel" / "incan_lang" / "src" / "lang"
    paths = [
        base / "builtins.rs",
        base / "surface" / "functions.rs",
        base / "surface" / "constructors.rs",
        base / "surface" / "math.rs",
        base / "surface" / "methods.rs",
    ]
    canonicals, aliases = _load_lang_items(paths)
    return canonicals.union(aliases)


def _load_stdlib_types() -> set[str]:
    repo_root = Path(__file__).resolve().parents[3]
    base = repo_root / "loaves" / "kernel" / "incan_lang" / "src" / "lang"
    paths = [
        base / "types" / "numerics.rs",
        base / "types" / "collections.rs",
        base / "types" / "stringlike.rs",
        base / "surface" / "types.rs",
        base / "derives.rs",
        base / "errors.rs",
    ]
    canonicals, aliases = _load_lang_items(paths)
    return canonicals.union(aliases)


def _fallback_keywords() -> list[str]:
    """Fallback keywords if registry parsing fails."""
    return [
        "and",
        "as",
        "async",
        "await",
        "break",
        "case",
        "class",
        "const",
        "continue",
        "crate",
        "def",
        "elif",
        "else",
        "enum",
        "false",
        "for",
        "from",
        "if",
        "import",
        "in",
        "is",
        "let",
        "match",
        "model",
        "mut",
        "newtype",
        "none",
        "not",
        "or",
        "pass",
        "pub",
        "python",
        "return",
        "rust",
        "self",
        "super",
        "trait",
        "true",
        "type",
        "while",
        "with",
        "yield",
    ]


def _keywords() -> Iterable[str]:
    keywords = _load_keywords_from_registry()
    # Extra doc-facing keywords not in the registry yet.
    extras = {"module", "tests", "test", "derive"}
    return sorted(set(keywords).union(_fallback_keywords(), extras))


class IncanLexer(PythonLexer):
    """Pygments lexer for the Incan programming language."""

    name = "Incan"
    aliases = ["incan", "incn"]
    filenames = ["*.incn"]
    mimetypes = ["text/x-incan"]

    flags = re.MULTILINE

    def __init__(self, **options):
        super().__init__(**options)
        self._incan_keywords = set(_keywords())
        self._incan_types = _load_stdlib_types()
        self._incan_functions = _load_stdlib_functions()

    def get_tokens_unprocessed(self, text, stack=("root",)):
        for index, token, value in super().get_tokens_unprocessed(text, stack=stack):
            if token is Name and value in self._incan_keywords:
                yield index, Keyword, value
                continue
            if token is Name and value in self._incan_types:
                yield index, Keyword.Type, value
                continue
            if token is Name and value in self._incan_functions:
                yield index, Name.Builtin, value
                continue
            if token is Name and value.startswith("assert_"):
                yield index, Name.Function, value
                continue
            if token is Token.Error and value == "?":
                yield index, Operator, value
                continue
            if token is Name and value[:1].isupper():
                yield index, Name.Class, value
                continue
            yield index, token, value


def register_incan_lexer() -> None:
    """Register Incan lexer with Pygments for MkDocs builds."""

    if "IncanLexer" in _mapping.LEXERS:
        return

    _mapping.LEXERS["IncanLexer"] = (
        __name__,
        IncanLexer.name,
        tuple(IncanLexer.aliases),
        tuple(IncanLexer.filenames),
        tuple(IncanLexer.mimetypes),
    )


__all__ = ["IncanLexer"]
