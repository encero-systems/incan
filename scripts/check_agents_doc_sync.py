#!/usr/bin/env python3
"""Check that AGENTS.md's "Available Skills and Agents" table matches the
actual contents of .agents/skills/.

This exists because AGENTS.md's skill table silently drifted from the real
skill set before (documenting 7 of 27 skills at one point) with nothing
catching it. Run locally via `make agents-doc-sync` / as part of
`make pre-commit-fast`; deliberately not wired into remote CI.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
AGENTS_MD = REPO_ROOT / "AGENTS.md"
SKILLS_DIR = REPO_ROOT / ".agents" / "skills"

# Skills that are internal building blocks, not meant to be invoked by name
# and therefore not expected to appear in the user-facing table. Empty for
# now -- every skill under .agents/skills/ is user-invocable today.
DOC_EXEMPT_SKILLS: set[str] = set()


def actual_skills() -> set[str]:
    return {
        child.name
        for child in SKILLS_DIR.iterdir()
        if child.is_dir() and (child / "SKILL.md").exists()
    }


def documented_skills() -> set[str]:
    text = AGENTS_MD.read_text()
    # Capture everything between "### Skills" and the next heading, so this
    # doesn't stop early at a blank line inside the section (e.g. the intro
    # sentence before the table).
    match = re.search(r"### Skills\n(.*?)\n#{1,3} ", text, re.DOTALL)
    if not match:
        return set()
    section = match.group(1)
    # Row cells look like `| \`/skill-name\`   | ... |`
    return set(re.findall(r"`/([a-z-]+)`", section))


def main() -> int:
    if not AGENTS_MD.exists():
        print(f"missing {AGENTS_MD}", file=sys.stderr)
        return 1
    if not SKILLS_DIR.is_dir():
        print(f"missing {SKILLS_DIR}", file=sys.stderr)
        return 1

    actual = actual_skills() - DOC_EXEMPT_SKILLS
    documented = documented_skills()

    undocumented = sorted(actual - documented)
    stale = sorted(documented - actual)

    if not undocumented and not stale:
        return 0

    if undocumented:
        print("skills present in .agents/skills/ but missing from AGENTS.md's skill table:")
        for name in undocumented:
            print(f"  {name}")
    if stale:
        print("skills documented in AGENTS.md's skill table but no longer present in .agents/skills/:")
        for name in stale:
            print(f"  {name}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
