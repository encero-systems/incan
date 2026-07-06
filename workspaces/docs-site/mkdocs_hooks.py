from __future__ import annotations

import html
import json
import re
import sys
from pathlib import Path

from mkdocs.plugins import event_priority

_BODY_TERM_LIMIT = 32
_EXCLUDED_SEARCH_PREFIXES = ("_snippets/", "snippets/")
_MERGED_SEARCH_PREFIXES = ("RFCs/",)
_MERGED_TEXT_TERM_LIMIT = 384
_TAG_RE = re.compile(r"<[^>]+>")
_TERM_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_./:+-]*")
_PATH_SPLIT_RE = re.compile(r"[/#_.:+-]+")

_STOP_WORDS = {
    "about",
    "after",
    "also",
    "and",
    "are",
    "because",
    "been",
    "but",
    "can",
    "could",
    "does",
    "for",
    "from",
    "has",
    "have",
    "how",
    "into",
    "its",
    "may",
    "must",
    "not",
    "that",
    "the",
    "their",
    "then",
    "there",
    "these",
    "this",
    "use",
    "used",
    "uses",
    "when",
    "where",
    "which",
    "while",
    "with",
    "without",
    "would",
    "you",
}


def _plain_search_text(value: str) -> str:
    value = _TAG_RE.sub(" ", value)
    value = html.unescape(value)
    return re.sub(r"\s+", " ", value).strip()


def _terms(value: str, *, limit: int | None = None) -> list[str]:
    seen: set[str] = set()
    terms: list[str] = []

    for match in _TERM_RE.finditer(value):
        raw = match.group(0)
        candidates = [raw, *_PATH_SPLIT_RE.split(raw)]
        for candidate in candidates:
            term = candidate.strip("._:+-/").lower()
            if len(term) < 3 or term in _STOP_WORDS or term in seen:
                continue
            seen.add(term)
            terms.append(term)
            if limit is not None and len(terms) >= limit:
                return terms

    return terms


def _location_terms(location: str) -> list[str]:
    location = location.removesuffix("/")
    return _terms(" ".join(_PATH_SPLIT_RE.split(location)))


def _unique_terms(terms: list[str], *, limit: int | None = None) -> list[str]:
    seen: set[str] = set()
    unique: list[str] = []

    for term in terms:
        if term in seen:
            continue
        seen.add(term)
        unique.append(term)
        if limit is not None and len(unique) >= limit:
            return unique

    return unique


def _base_location(location: str) -> str:
    return location.split("#", 1)[0]


def _should_merge_to_page(location: str) -> bool:
    return any(location.startswith(prefix) for prefix in _MERGED_SEARCH_PREFIXES)


def _compact_search_document(document: dict) -> dict | None:
    location = str(document.get("location", ""))
    if location.startswith(_EXCLUDED_SEARCH_PREFIXES):
        return None

    location_terms = _location_terms(location)
    title_terms = _terms(str(document.get("title", "")))
    body_terms = _terms(
        _plain_search_text(str(document.get("text", ""))),
        limit=_BODY_TERM_LIMIT,
    )
    existing_tags = [
        str(tag).lower()
        for tag in document.get("tags", [])
        if isinstance(tag, (str, int, float, bool))
    ]

    document["tags"] = sorted(set(existing_tags))
    document["text"] = " ".join(
        _unique_terms([*location_terms, *title_terms, *body_terms])
    )
    return document


def _merge_search_document(target: dict, source: dict) -> None:
    target["text"] = " ".join(
        _unique_terms(
            [
                *str(target.get("text", "")).split(),
                *str(source.get("text", "")).split(),
            ],
            limit=_MERGED_TEXT_TERM_LIMIT,
        )
    )
    target["tags"] = sorted(
        set(target.get("tags", [])) | set(source.get("tags", []))
    )

    source_location = str(source.get("location", ""))
    if "#" not in source_location:
        target["title"] = source.get("title", target.get("title", ""))
        target["boost"] = source.get("boost", target.get("boost"))


def _compact_search_documents(documents: list[dict]) -> list[dict]:
    compacted: list[dict] = []
    merged_pages: dict[str, dict] = {}

    for raw_document in documents:
        document = _compact_search_document(raw_document)
        if document is None:
            continue

        location = str(document.get("location", ""))
        if not _should_merge_to_page(location):
            compacted.append(document)
            continue

        base_location = _base_location(location)
        target = merged_pages.get(base_location)
        if target is None:
            target = {**document, "location": base_location}
            merged_pages[base_location] = target
            compacted.append(target)
            continue

        _merge_search_document(target, document)

    return compacted


def on_config(config):  # noqa: D401 - MkDocs hook signature
    """Register custom Pygments lexers before Markdown rendering."""
    root = Path(__file__).resolve().parent
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))

    from utils.incan_pygments import register_incan_lexer

    register_incan_lexer()
    return config


@event_priority(-100)
def on_post_build(config):  # noqa: D401 - MkDocs hook signature
    """Reduce client-side search work while retaining title, tag, and anchor search."""
    index_path = Path(config.site_dir) / "search" / "search_index.json"
    if not index_path.exists():
        return

    search_index = json.loads(index_path.read_text())
    search_index["docs"] = _compact_search_documents(search_index.get("docs", []))

    index_path.write_text(
        json.dumps(search_index, sort_keys=True, separators=(",", ":")),
    )
