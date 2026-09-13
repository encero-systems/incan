#!/usr/bin/env python3
"""Check that every deferred parity-corpus row still names an OPEN owning issue.

The parity corpus is acceptance evidence: a row dispositioned `Unsupported` or
`Unavailable` asserts that a capability is not yet available, on the authority of
the issue it names. `tests/support/parity_corpus.rs` already rejects
`owning_issue: 0` and requires the reason text to cite the number -- but it never
checks that the issue is still open, and it cannot, because the suite runs
offline.

So the row survives its own owner. Measured 2026-09-13: eight rows named four
issues that had already closed, one of them (#1156, five rows) completed a
fortnight earlier, and the suite stayed green throughout. Slice 9's finish line
is a matrix that is green or explicitly migrated; a matrix that cannot notice its
own stale rows cannot prove that about itself.

Run deliberately -- `make parity-corpus-owners` -- not from the test suite and
not from remote CI, because it needs network and a `gh` login. A closed owner is
not automatically a defect: the capability may genuinely have landed, in which
case the row should be re-evaluated, or the work may have moved to a new issue,
in which case the row should name that one.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CORPUS = REPO_ROOT / "tests" / "parity_corpus_tests.rs"

# `parity-987-unsupported-no-issue` is a deliberate red-state fixture proving the
# corpus rejects a missing owner. It is never a real deferral.
SENTINEL_OWNERS = {0}


def owning_issues() -> dict[int, int]:
    """Return every owning issue the corpus cites, mapped to how many rows cite it."""
    text = CORPUS.read_text(encoding="utf-8")
    counts: dict[int, int] = {}
    for match in re.finditer(r"owning_issue:\s*(\d+)", text):
        number = int(match.group(1))
        if number in SENTINEL_OWNERS:
            continue
        counts[number] = counts.get(number, 0) + 1
    return counts


def issue_states(numbers: list[int]) -> dict[int, tuple[str, str]]:
    """Return `{number: (state, state_reason)}` for each issue, via the `gh` CLI."""
    states: dict[int, tuple[str, str]] = {}
    for number in numbers:
        completed = subprocess.run(
            ["gh", "issue", "view", str(number), "--json", "state,stateReason"],
            capture_output=True,
            text=True,
            check=False,
        )
        if completed.returncode != 0:
            message = completed.stderr.strip() or "gh returned no detail"
            print(f"error: could not read issue #{number}: {message}", file=sys.stderr)
            raise SystemExit(2)
        payload = json.loads(completed.stdout)
        states[number] = (payload.get("state", "UNKNOWN"), payload.get("stateReason") or "")
    return states


def main() -> int:
    counts = owning_issues()
    if not counts:
        print("No deferred parity rows name an owning issue.")
        return 0

    states = issue_states(sorted(counts))
    stale = {number: counts[number] for number, (state, _) in states.items() if state != "OPEN"}

    for number in sorted(counts):
        state, reason = states[number]
        mark = "stale" if state != "OPEN" else "ok"
        detail = f" ({reason.lower()})" if reason else ""
        print(f"  {mark:>5}  #{number:<6} {counts[number]:>2} row(s)  {state}{detail}")

    if not stale:
        print(f"\nAll {len(counts)} owning issue(s) are still open.")
        return 0

    rows = sum(stale.values())
    print(
        f"\n{rows} parity row(s) defer to {len(stale)} closed issue(s): "
        + ", ".join(f"#{number}" for number in sorted(stale)),
        file=sys.stderr,
    )
    print(
        "Re-evaluate each row: the capability may now be available, or the work may "
        "have moved to an issue that is still open.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
