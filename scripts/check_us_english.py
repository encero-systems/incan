#!/usr/bin/env python3
"""Check tracked text for UK spellings; US English is the repository's standard (#1561).

The table below maps UK base forms to US ones and the inflections are generated from them, so `behaviour`
covers `behaviours`, `behavioural` and `misbehaviour` is listed on its own. The `-ise` family is an explicit
word list, never a bare suffix rule: `advertise`, `compromise`, `otherwise`, `precise`, `promise` and the rest
of the words that end in `-ise` in both dialects are simply absent from it. Matching is case-preserving
(`colour`, `Colour`, `COLOUR`) and word-bounded in a way that follows snake_case (`_catalogue_`) and CamelCase
word starts (`BackgroundColour`) but never a run of letters inside a CamelCase word (`EntityRef` holds no
`tyre`). Legitimate UK spellings -- upstream identifiers, quoted third-party text, generated or frozen files --
live in scripts/check_us_english.allow: one tab-separated `path-or-*`, `token-or-*`, `reason` per line.

`--check` (the default) lists `path:line: word → replacement` and exits 1 when anything is left; `--fix`
rewrites the files in place, prose and identifiers alike, and prints a per-file count.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import subprocess
import sys

# fmt: off
# UK -> US base forms. Regular verbs and nouns get -s/-d/-ed/-ing/-er/-ation/-able generated; the second table
# lists forms that do not inflect regularly (a doubled consonant, a noun with its own plural) one by one. The
# cancel family (`cancelled`, `cancelling`) is deliberately absent: the owner keeps that spelling (#1561).
ISE_STEMS = (
    "anonymise", "authorise", "canonicalise", "capitalise", "categorise", "centralise", "characterise",
    "customise", "deserialise", "digitise", "emphasise", "finalise", "formalise", "generalise", "harmonise",
    "initialise", "internalise", "itemise", "linearise", "localise", "materialise", "maximise", "memoise",
    "minimise", "modernise", "modularise", "monomorphise", "normalise", "optimise", "organise", "parameterise",
    "parenthesise", "prioritise", "quantise", "randomise", "rationalise", "realise", "recognise", "reinitialise",
    "reorganise", "reserialise", "sanitise", "scrutinise", "serialise", "specialise", "stabilise", "standardise",
    "summarise", "synchronise", "synthesise", "systematise", "tokenise", "trivialise", "uninitialise",
    "unrecognise", "unauthorise", "unoptimise", "unnormalise", "unsynchronise", "unserialise", "utilise",
    "vectorise", "virtualise", "visualise",
)
OUR_STEMS = (
    "armour", "behaviour", "candour", "clamour", "colour", "decolour", "demeanour", "discolour", "dishonour", "endeavour",
    "favour", "fervour", "flavour", "harbour", "honour", "humour", "labour", "misbehaviour", "neighbour", "odour",
    "parlour", "recolour", "rigour", "rumour", "saviour", "savour", "splendour", "uncolour", "valour", "vapour",
    "vigour",
)
REGULAR = {
    "catalogue": "catalog", "analogue": "analog", "centre": "center", "metre": "meter", "litre": "liter",
    "fibre": "fiber", "calibre": "caliber", "kilometre": "kilometer", "millimetre": "millimeter", "datacentre": "datacenter",
    "centimetre": "centimeter", "artefact": "artifact", "judgement": "judgment",
    "acknowledgement": "acknowledgment", "abridgement": "abridgment", "defence": "defense", "offence": "offense",
    "pretence": "pretense", "licence": "license", "practise": "practice", "grey": "gray", "analyse": "analyze",
    "paralyse": "paralyze", "catalyse": "catalyze", "encyclopaedia": "encyclopedia",
    "sceptic": "skeptic", "enquire": "inquire", "enquiry": "inquiry", "aluminium": "aluminum",
    "speciality": "specialty", "cosy": "cozy", "mould": "mold", "plough": "plow", "tyre": "tire",
}
IRREGULAR = {
    "programme": "program", "programmes": "programs", "whilst": "while", "amongst": "among",
    "anticlockwise": "counterclockwise", "ageing": "aging", "learnt": "learned", "spelt": "spelled",
    "enquiries": "inquiries", "specialities": "specialties", "analyses": None, "greys": "grays",
    "manoeuvre": "maneuver", "manoeuvres": "maneuvers", "manoeuvred": "maneuvered", "manoeuvring": "maneuvering",
    "labelled": "labeled", "labelling": "labeling", "unlabelled": "unlabeled", "relabelled": "relabeled", "relabelling": "relabeling",
    "mislabelled": "mislabeled", "modelled": "modeled", "modelling": "modeling", "modeller": "modeler",
    "remodelled": "remodeled", "signalled": "signaled", "signalling": "signaling", "unsignalled": "unsignaled",
    "travelled": "traveled", "travelling": "traveling", "traveller": "traveler", "totalled": "totaled",
    "totalling": "totaling", "marshalled": "marshaled", "marshalling": "marshaling", "unmarshalled": "unmarshaled",
    "unmarshalling": "unmarshaling", "channelled": "channeled", "channelling": "channeling", "levelled": "leveled",
    "levelling": "leveling", "fuelled": "fueled", "fuelling": "fueling", "tunnelled": "tunneled",
    "tunnelling": "tunneling", "dialled": "dialed", "dialling": "dialing", "equalled": "equaled",
    "equalling": "equaling", "funnelled": "funneled", "funnelling": "funneling", "spiralled": "spiraled",
    "spiralling": "spiraling", "panelled": "paneled", "pencilled": "penciled", "counsellor": "counselor",
    "jewellery": "jewelry", "enrol": "enroll", "enrols": "enrolls", "enrolment": "enrollment",
    "enrolments": "enrollments", "fulfil": "fulfill", "fulfils": "fulfills", "fulfilment": "fulfillment",
    "fulfilments": "fulfillments", "instalment": "installment", "instalments": "installments",
    "skilful": "skillful", "wilful": "willful", "distil": "distill", "instil": "instill", "maths": "math",
    "favourite": "favorite", "favourites": "favorites", "favourable": "favorable", "favourably": "favorably",
    "honourable": "honorable", "honourably": "honorably", "unhonourable": "unhonorable", "colourful": "colorful",
    "colourless": "colorless",
    "multicoloured": "multicolored", "neighbourhood": "neighborhood", "neighbourhoods": "neighborhoods",
    "behavioural": "behavioral", "behaviourally": "behaviorally", "labourer": "laborer", "labourers": "laborers",
    "humourless": "humorless", "recognisable": "recognizable", "serialisable": "serializable",
    "customisable": "customizable", "optimiser": "optimizer", "organiser": "organizer", "tokeniser": "tokenizer",
    "synthesiser": "synthesizer", "analyser": "analyzer", "analysers": "analyzers", "centred": "centered",
    "centring": "centering", "practising": "practicing", "greyed": "grayed", "greying": "graying",
    "greyscale": "grayscale", "sceptical": "skeptical", "scepticism": "skepticism",
}
# fmt: on


def spellings() -> dict[str, str]:
    """Expand the base tables into every lowercase UK form and its US replacement."""
    table: dict[str, str] = {}
    for stem in ISE_STEMS:
        base = stem[:-3]
        for uk, us in (("ise", "ize"), ("ises", "izes"), ("ised", "ized"), ("ising", "izing"), ("iser", "izer"),
                       ("isers", "izers"), ("isation", "ization"), ("isations", "izations"), ("isable", "izable"),
                       ("isably", "izably")):
            table[base + uk] = base + us
    for stem in OUR_STEMS:
        base = stem[:-3]
        for uk, us in (("our", "or"), ("ours", "ors"), ("oured", "ored"), ("ouring", "oring")):
            table[base + uk] = base + us
    for uk, us in REGULAR.items():
        if uk == us:
            continue
        table[uk], table[uk + "s"] = us, us + "s"
        if uk.endswith("y"):
            continue  # `-ies`, `-ied` forms are listed one by one below
        stem_uk, stem_us = uk.rstrip("e"), us.rstrip("e")
        table[stem_uk + "ed"], table[stem_uk + "ing"] = stem_us + "ed", stem_us + "ing"
    for uk, us in IRREGULAR.items():
        if us is None:
            table.pop(uk, None)  # `analyses` is the plural of `analysis` in both dialects
        else:
            table[uk] = us
    return table


TABLE = spellings()
LETTERS = re.compile(r"[^\W\d_]+")
SKIPPED_TREES = ("workspaces/docs-site/docs/shared/",)
# The guard, its tests and its allow file spell the UK words on purpose.
SELF = ("scripts/check_us_english.py", "scripts/test_check_us_english.py", "scripts/check_us_english.allow")
SKIPPED_SUFFIXES = (".lock", ".jsonl", ".png", ".jpg", ".jpeg", ".webp", ".gif", ".ico", ".woff", ".woff2", ".pdf")


@dataclass(frozen=True)
class ExceptionRule:
    """A documented exception: a path (exact, `/**` subtree or `*`) and a token (a UK word, a longer phrase, or `*`)."""

    path: str
    token: str
    reason: str

    def covers(self, path: str) -> bool:
        """Keep subtree exceptions beneath their slash boundary, never sibling prefixes."""
        return self.path in ("*", path) or (self.path.endswith("/**") and path.startswith(self.path[:-2]))

    def matches(self, path: str, line: str, start: int, end: int) -> bool:
        """A one-word token matches the hit in any case; a longer token must enclose the hit on the line, exact case."""
        if not self.covers(path):
            return False
        if self.token == "*" or self.token.lower() == line[start:end].lower():
            return True
        position = line.find(self.token)
        while position != -1:
            if position <= start and end <= position + len(self.token):
                return True
            position = line.find(self.token, position + 1)
        return False


def read_allowlist(path: Path) -> list[ExceptionRule]:
    """Reject malformed configuration instead of silently disabling intended checks."""
    result = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t", 2)
        if len(fields) != 3 or not all(field.strip() for field in fields):
            raise ValueError(f"{path}:{number}: expected path, token and reason separated by tabs")
        result.append(ExceptionRule(*(field.strip() for field in fields)))
    return result


def tracked_text_files(root: Path) -> list[Path]:
    """Every git-tracked file that is not an asset, a lock file, a log, binary, or the guard itself."""
    listing = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=True)
    result = []
    for name in listing.stdout.decode("utf-8").split("\0"):
        if not name or name in SELF or name.startswith(SKIPPED_TREES) or name.endswith(SKIPPED_SUFFIXES):
            continue
        path = root / name
        if path.is_file() and b"\0" not in path.read_bytes()[:8192]:
            result.append(path)
    return result


def respell(word: str) -> str | None:
    """Return the US spelling in the same case pattern, or None for a word that is not in the table or mixed-case."""
    replacement = TABLE.get(word.lower())
    if replacement is None or len(word) < 2:
        return None
    if word.islower():
        return replacement
    if word.isupper():
        return replacement.upper()
    if word[0].isupper() and word[1:].islower():
        return replacement.capitalize()
    return None


def words(line: str):
    """Yield (start, word) for every letter run, split again at CamelCase word starts (a lowercase-to-uppercase step).

    Letters bounded by anything else -- digits, underscores, punctuation, whitespace -- form a word, so snake_case parts
    are words; a run of letters inside a CamelCase word is not, which is what keeps `tyre` out of `EntityRef`.
    """
    for run in LETTERS.finditer(line):
        text, start = run.group(0), run.start()
        piece = 0
        for index in range(1, len(text)):
            if text[index - 1].islower() and text[index].isupper():
                yield start + piece, text[piece:index]
                piece = index
        yield start + piece, text[piece:]


def hits(line: str) -> list[tuple[int, int, str, str]]:
    """Every (start, end, word, replacement) in one line, left to right and non-overlapping."""
    result = []
    for start, word in words(line):
        replacement = respell(word)
        if replacement is not None:
            result.append((start, start + len(word), word, replacement))
    return result


def main(argv: list[str] | None = None) -> int:
    """Report or rewrite UK spellings; configuration failures use exit status 2."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--allow", type=Path, help="override the repository's scripts/check_us_english.allow")
    parser.add_argument("--fix", action="store_true", help="rewrite files in place instead of reporting")
    parser.add_argument("paths", nargs="*", type=Path, help="limit the scan to these tracked files")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        allowed = read_allowlist(args.allow or root / "scripts/check_us_english.allow")
        files = tracked_text_files(root)
        if args.paths:
            wanted = {path.resolve() for path in args.paths}
            files = [path for path in files if path in wanted]
        findings: list[tuple[str, int, str, str]] = []
        rewritten = 0
        for path in files:
            relative = path.relative_to(root).as_posix()
            if any(rule.covers(relative) and rule.token == "*" for rule in allowed):
                continue
            with open(path, encoding="utf-8", newline="") as handle:
                lines = handle.read().split("\n")
            changed = 0
            for index, line in enumerate(lines):
                found = [hit for hit in hits(line) if not any(rule.matches(relative, line, *hit[:2]) for rule in allowed)]
                if not found:
                    continue
                for _, _, word, replacement in found:
                    findings.append((relative, index + 1, word, replacement))
                if args.fix:
                    pieces, cursor = [], 0
                    for start, end, _, replacement in found:
                        pieces.append(line[cursor:start] + replacement)
                        cursor = end
                    lines[index] = "".join(pieces) + line[cursor:]
                    changed += len(found)
            if changed:
                with open(path, "w", encoding="utf-8", newline="") as handle:
                    handle.write("\n".join(lines))
                rewritten += 1
                print(f"{relative}: {changed}")
        if args.fix:
            print(f"us-english: rewrote {len(findings)} spellings in {rewritten} files")
            return 0
        if findings:
            print(f"{len(findings)} UK spellings remain (US English is the standard, see AGENTS.md):")
            for relative, number, word, replacement in findings:
                print(f"  {relative}:{number}: {word} → {replacement}")
            return 1
        print(f"us-english audit passed: {len(files)} files scanned")
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"us-english audit: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
