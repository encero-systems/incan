#!/usr/bin/env python3
"""Claude Code hook dispatcher for this repo.

Reuses this maintainer's existing Codex hook scripts (~/.codex/hooks/) so
Claude Code and Codex enforce the same Ralph-loop hygiene and Incan/Rust-
boundary review policy from one source of truth, instead of drifting apart
as two hand-maintained copies.

Fails open, silently, if ~/.codex/hooks isn't present on this machine -- e.g.
a collaborator who clones this repo but doesn't run Codex. This hook exists
to keep tools in sync for people who already run both; it must not become a
hard dependency on Codex for everyone else, and it must never block a turn
just because it couldn't reach its own dependency.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

RALPH_DISPATCH = Path.home() / ".codex" / "hooks" / "ralph_state.py"


def main() -> int:
    if not RALPH_DISPATCH.exists():
        return 0

    raw = sys.stdin.read()

    try:
        result = subprocess.run(
            [sys.executable, str(RALPH_DISPATCH)],
            input=raw,
            capture_output=True,
            text=True,
            timeout=25,
            check=False,
        )
    except Exception as exc:
        sys.stderr.write(f"claude_hooks/dispatch.py: failed open ({exc})\n")
        return 0

    stdout = result.stdout.strip()
    if not stdout:
        return 0

    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError:
        return 0

    sys.stdout.write(json.dumps(payload))

    # The underlying Codex scripts emit the flat {"decision": "block", "reason": ...}
    # shape on stdout for a Stop-hook block, which Claude Code's Stop hook also
    # accepts directly. Also satisfy the exit-code-2 + stderr contract as cheap
    # redundancy, since that detail wasn't independently confirmed at write time --
    # see the note in .claude/settings.json history / commit message.
    if payload.get("decision") == "block":
        reason = payload.get("reason") or "blocked by shared Codex/Claude hook policy"
        sys.stderr.write(reason + "\n")
        return 2

    return 0


if __name__ == "__main__":
    sys.exit(main())
