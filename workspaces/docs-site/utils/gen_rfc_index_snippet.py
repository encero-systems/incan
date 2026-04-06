from __future__ import annotations

import logging
import os
import re
from pathlib import Path

import mkdocs_gen_files
from mkdocs.exceptions import ConfigurationError

log = logging.getLogger("mkdocs.plugins.gen-files")

DOCS_DIR = Path(__file__).resolve().parents[1] / "docs"
RFC_DIR = DOCS_DIR / "RFCs"
CLOSED_DIR = RFC_DIR / "closed"
IMPLEMENTED_DIR = CLOSED_DIR / "implemented"
SUPERSEDED_DIR = CLOSED_DIR / "superseded"
REJECTED_DIR = CLOSED_DIR / "rejected"

DOCS_BASE_PREFIX = Path("/" + os.environ.get("INCAN_DOCS_BASE_PREFIX", "").strip())

RFC_WIDTH = 9
STATUS_WIDTH = 11
TRACK_WIDTH = 20
TITLE_WIDTH = 120 - (2 + RFC_WIDTH + 3 + STATUS_WIDTH + 3 + TRACK_WIDTH + 3 + 2)


def _with_base_prefix(path: str) -> str:
    """Prefix a site-root path with the configured docs base prefix."""
    return (DOCS_BASE_PREFIX / path.lstrip("/")).as_posix()


def _extract_rfc_metadata(md_path: Path, need_status: bool) -> tuple[str, str]:
    """Extract title and optionally status from RFC file in one read.
    
    Returns:
        (title, status) where status is "Unknown" if need_status=False or not found.
    """
    text = md_path.read_text(encoding="utf-8", errors="replace")
    
    title = None
    status = "Unknown"
    
    for line in text.splitlines():
        # Extract title (first H1)
        if title is None:
            if m := re.match(r"^#\s+(.+?)\s*$", line):
                title = m.group(1).strip()
        
        # Extract status (only if needed for open RFCs)
        if need_status:
            if m := re.match(r"^\s*(?:-\s*)?\*\*Status(?::)?\*\*:?\s*(.+?)\s*$", line):
                status = m.group(1).strip()
                break
            if m := re.match(r"^\s*-\s*Status:\s*(.+?)\s*$", line):
                status = m.group(1).strip()
                break
        
        # Early exit if we have both
        if title and (not need_status or status != "Unknown"):
            break
    
    return (title or md_path.stem, status)


def _rfc_id_from_filename(filename: str) -> str:
    """extract the RFC id from a filename
    "013_rust_crate_dependencies.md" -> "013"
    """
    m = re.match(r"^(\d+)", filename)
    return m.group(1) if m else filename


def _escape_pipes(s: str) -> str:
    """Keep markdown tables valid by escaping pipes."""
    return s.replace("|", "\\|")

def _strip_rfc_prefix(title: str, rfc_id: str) -> str:
    """Convert titles like 'RFC 001: Test Fixtures' to 'Test Fixtures' for display."""
    title = title.strip()
    m = re.match(rf"^RFC\s+{re.escape(rfc_id)}\s*:\s*(.+)$", title, flags=re.IGNORECASE)
    return m.group(1).strip() if m else title


def _sorted_md_files(dir_path: Path) -> list[Path]:
    """Sort markdown files in a directory numerically (by their name)."""
    if not dir_path.exists():
        return []
    files = [
        p
        for p in dir_path.glob("*.md")
        if p.is_file() and p.name.lower() != "index.md"
    ]
    # Stable numeric-ish ordering: filenames are zero-padded.
    return sorted(files, key=lambda p: p.name)


def _collect_rfc_md_files() -> list[Path]:
    """Collect RFC markdown files under `RFCs/` (including closed/ subfolders).
    
    Filters out index.md and TEMPLATE.md. Returns unsorted (will be sorted by RFC ID later).
    """
    if not RFC_DIR.exists():
        return []
    files: list[Path] = []
    for p in RFC_DIR.rglob("*.md"):
        if not p.is_file():
            continue
        name_lower = p.name.lower()
        if name_lower == "index.md" or p.name == "TEMPLATE.md":
            continue
        files.append(p)
    return files


def _collect_rows() -> list[tuple[str, str, str, str, str]]:
    """Collect RFCs and return rows: (rfc_id, title, status, track, url)."""
    rows: list[tuple[str, str, str, str, str]] = []

    # One pass over all RFC markdown files; track/status are inferred from location.
    for p in _collect_rfc_md_files():
        rel = p.relative_to(RFC_DIR)

        # Determine track and whether we need to extract status from file
        match rel.parts:
            case ("closed", "implemented", *_):
                need_status = False
                status = "Done"
                track = "closed / implemented"
            case ("closed", "superseded", *_):
                need_status = False
                status = "Superseded"
                track = "closed / superseded"
            case ("closed", "rejected", *_):
                need_status = False
                status = "Rejected"
                track = "closed / rejected"
            case _:
                need_status = True
                track = "proposed / active"
                status = ""  # Will be extracted below

        # Extract metadata in one file read
        raw_title, extracted_status = _extract_rfc_metadata(p, need_status)
        if need_status:
            status = extracted_status

        rfc_id = _rfc_id_from_filename(p.name)
        title = _strip_rfc_prefix(raw_title, rfc_id)

        # URL mirrors doc path, but uses directory URLs (`.../<name>/`).
        url_path = f"/RFCs/{rel.with_suffix('').as_posix()}/"
        url = _with_base_prefix(url_path)
        rows.append((rfc_id, title, status, track, url))

    # Sort by RFC id (string numeric sort works because ids are zero-padded)
    rows.sort(key=lambda r: r[0])
    return rows


def _render_reference_links(rows: list[tuple[str, str, str, str, str]]) -> str:
    """Render reference-style RFC links for reuse across docs pages."""
    lines: list[str] = []
    lines.append("<!-- THIS FILE IS AUTOGENERATED. DO NOT EDIT BY HAND. -->")
    lines.append("")
    lines.append("<!-- RFC reference links for reuse: '[RFC 018]' or '[RFC 018: Title][RFC 018]' -->")
    for rfc_id, _title, _status, _track, url in rows:
        lines.append(f"[RFC {rfc_id}]: {url}")
    lines.append("")
    return "\n".join(lines)


def _render_table(rows: list[tuple[str, str, str, str, str]]) -> str:
    """Render the RFCs index table."""
    lines: list[str] = []
    lines.append("<!-- THIS FILE IS AUTOGENERATED. DO NOT EDIT BY HAND. -->")
    lines.append("")
    lines.append("<!-- markdownlint-disable MD013 MD060 MD053 MD033 -->")

    lines.append("<!-- Include RFC reference links (DRY: reuse rfcs_refs.md) -->")
    lines.append("--8<-- \"_snippets/rfcs_refs.md\"")
    lines.append("")

    lines.append("<!-- RFCs index table -->")
    lines.append("")
    lines.append("<!-- RFC index filter UI -->")
    lines.append("")
    lines.append('<div class="rfc-index-filter">')
    lines.append('  <label>')
    lines.append('    Filter RFCs: <input type="search" data-rfc-filter="rfcs-index" placeholder="e.g. 018, testing, superseded">')
    lines.append('  </label>')
    lines.append('  <p><small>Tip: press <code>Esc</code> to clear.</small></p>')
    lines.append("</div>")
    lines.append("")
    # NOTE: Markdown tables are not parsed inside raw HTML blocks unless explicitly enabled.
    # `md_in_html` is enabled in mkdocs.yml, so `markdown="1"` ensures the table renders correctly.
    lines.append('<div data-rfc-table="rfcs-index" markdown="1">')
    lines.append("")
    lines.append(f"| {'RFC':<{RFC_WIDTH}} | {'Status':<{STATUS_WIDTH}} | {'Track':<{TRACK_WIDTH}} | {'Title':<{TITLE_WIDTH}} |")
    lines.append(f"| {'':-<{RFC_WIDTH-1}}: | {'':-<{STATUS_WIDTH}} | {'':-<{TRACK_WIDTH}} | {'':-<{TITLE_WIDTH}} |")
    for rfc_id, title, status, track, url in rows:
        safe_title = _escape_pipes(title)
        safe_status = _escape_pipes(status)
        safe_track = _escape_pipes(track)
        lines.append(f"| {'[RFC ' + rfc_id + ']':<{RFC_WIDTH}} | {safe_status:<{STATUS_WIDTH}} | {safe_track:<{TRACK_WIDTH}} | {safe_title:<{TITLE_WIDTH}} |")
    lines.append("")
    lines.append("</div>")
    lines.append("<!-- markdownlint-enable MD013 MD060 MD053 MD033 -->")

    return "\n".join(lines)

def main() -> None:
    """Main entry point for generating the RFCs index snippet."""
    prefix = os.environ.get('INCAN_DOCS_BASE_PREFIX', '(unset)')
    print(f"[gen-files] RFC Generator starting with INCAN_DOCS_BASE_PREFIX={prefix}")
    log.info("RFC Generator starting with INCAN_DOCS_BASE_PREFIX=%s", prefix)
    rows = _collect_rows()
    content = _render_table(rows)
    refs = _render_reference_links(rows)

    rel_path = "_snippets/tables/rfcs_index.md"
    refs_rel_path = "_snippets/rfcs_refs.md"

    # Write to mkdocs virtual filesystem first (used during build)
    try:
        with mkdocs_gen_files.open(rel_path, "w") as f:
            f.write(content)
        with mkdocs_gen_files.open(refs_rel_path, "w") as f:
            f.write(refs)
    except ConfigurationError:
        # Not running via mkdocs (standalone execution)
        pass
    
    # ALSO write physical files so pymdownx.snippets can find them.
    # During CI, these will have version prefixes; locally they won't.
    (DOCS_DIR / "_snippets" / "tables").mkdir(parents=True, exist_ok=True)
    
    refs_file = DOCS_DIR / refs_rel_path
    (DOCS_DIR / rel_path).write_text(content, encoding="utf-8")
    refs_file.write_text(refs, encoding="utf-8")
    
    # Log first few lines to verify prefix (visible in mkdocs build output)
    preview = "\n".join(refs.splitlines()[:5])
    prefix = os.environ.get('INCAN_DOCS_BASE_PREFIX', '(unset)')
    print(f"[gen-files] Wrote {refs_file} with DOCS_BASE_PREFIX={prefix}")
    print("[gen-files] Preview:")
    print(preview)
    log.info("Wrote %s with DOCS_BASE_PREFIX=%s", refs_file, prefix)
    log.info("Preview:\n%s", preview)


# Configure logging for standalone execution
if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format='%(levelname)s - %(message)s')

# Always run - mkdocs-gen-files imports this script, so main() must execute at module level
main()
