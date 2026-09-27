#!/usr/bin/env python3
"""Lint ``crates/wl-app/ui/public/wl.css``.

Why this is a script and not a test
-----------------------------------
A Rust test can ``include_str!`` the stylesheet and check it, but the check
that matters most here is a DIFF against a known-good baseline, and "the
previous baseline" is a git question, not a compile-time question. It is
also a check about text a browser will parse and a human will read, which
is squarely a linter's job.

Why the baseline diff is the important part
------------------------------------------
This file was shipped with a real dark-mode bug that four other checks all
passed over. A stray ``[data-theme="light"] `` prefix sat on a line of its
own; the CSS parser bound it to the *next* rule, so ``.wl-section`` — the
card container behind every Settings section — silently became a
light-mode-only rule. In dark mode those cards lost their background,
border, radius, padding and margin.

The reason a class-existence lint cannot see this is the point: after the
parser absorbs the prefix, ``.wl-section`` is still **defined**, still
**used**, still **unique** and still **not dead**. Every one of those
checks passes. The only thing that changed is the selector's *scope*, and
nothing short of comparing scopes against a baseline notices that.

So ``--against`` is the load-bearing flag.

Usage
-----
    scripts/css-lint.py                    # diff against HEAD
    scripts/css-lint.py --against main     # diff against a branch
    scripts/css-lint.py --against ''       # skip the diff; checks 1-4, 6-7

Exit code 0 = clean, 1 = at least one finding.
"""

from __future__ import annotations

import argparse
import collections
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CSS = REPO / "crates/wl-app/ui/public/wl.css"
UI_SRC = REPO / "crates/wl-app/ui/src"

# The token PRD delta 173 is about. It is in the file's own prose, so it can
# only be found by reading the stripped body -- and it must never be a
# declaration again.
BANNED_TOKENS = ("--wl-text-tertiary",)

COMMENT = re.compile(r"/\*.*?\*/", re.S)
# A theme-qualified prefix: `[data-theme="light"]`, with or without a class
# after it.
THEME_PREFIX = re.compile(r'^\[data-theme\s*=\s*"[a-z]+"\]\s*$')


class Finding:
    def __init__(self, check: str, where: str, message: str) -> None:
        self.check = check
        self.where = where
        self.message = message

    def __str__(self) -> str:
        return f"[{self.check}] {self.where}\n    {self.message}"


def line_of(text: str, index: int) -> int:
    return text.count("\n", 0, index) + 1


def strip_comments(css: str) -> str:
    """Blank out comments, preserving line numbering.

    Replaced with newlines rather than removed so that a reported line
    number is the line a human will actually see.
    """
    return COMMENT.sub(lambda m: "\n" * m.group(0).count("\n"), css)


KEYFRAMES = re.compile(r"@keyframes[^{]*\{(?:[^{}]*\{[^{}]*\})*[^{}]*\}", re.S)


def selectors(clean: str) -> list[str]:
    """Every rule selector, normalised.

    ``@keyframes`` bodies are removed first: their ``from`` / ``to`` step
    selectors are not class selectors, and leaving them in reported
    ``from is defined 4 times`` for every keyframe in the file.
    """
    out = []
    for match in re.finditer(r"([^{}]+)\{", KEYFRAMES.sub("", clean)):
        sel = " ".join(match.group(1).split())
        if sel and not sel.startswith("@"):
            out.append(sel)
    return out


def check_tokens(clean: str) -> list[Finding]:
    """1. Every ``var(--x)`` has a ``--x:`` definition."""
    defined = set(re.findall(r"(--[a-z0-9-]+)\s*:", clean))
    used = set(re.findall(r"var\(\s*(--[a-z0-9-]+)", clean))
    found = []
    for token in sorted(used - defined):
        found.append(
            Finding("undefined-token", "wl.css", f"var({token}) is used but never defined")
        )
    return found


def check_duplicate_selectors(clean: str) -> list[Finding]:
    """2. No selector is defined twice.

    Compared on the FULL selector text, so ``.x:hover`` and
    ``.x[aria-pressed]`` are variants rather than redefinitions.
    """
    counts = collections.Counter(selectors(clean))
    return [
        Finding("duplicate-selector", "wl.css", f"{sel!r} is defined {n} times")
        for sel, n in sorted(counts.items())
        if n > 1
    ]


def check_dangling_prefix(raw: str) -> list[Finding]:
    """3. No theme-qualified prefix standing alone on a line.

    This is the check that would have caught the 2026-09-27 dark-mode bug.
    It has to run on the RAW text: once the parser absorbs a bare prefix
    into the following rule, the result is a valid rule and nothing
    downstream can tell that is what happened.
    """
    found = []
    for number, line in enumerate(raw.split("\n"), start=1):
        if THEME_PREFIX.match(line):
            found.append(
                Finding(
                    "dangling-theme-prefix",
                    f"wl.css:{number}",
                    f"{line.strip()!r} has no declaration block, so the CSS parser "
                    "binds it to the NEXT rule and silently restricts that rule to "
                    "light mode. Delete the prefix.",
                )
            )
    return found


def check_banned_tokens(clean: str) -> list[Finding]:
    """4. Retired tokens must stay retired (PRD delta 173)."""
    return [
        Finding(
            "retired-token",
            "wl.css",
            f"{token} is declared or used again; delta 173 removed it because "
            "it was referenced thirteen times and defined in none of them",
        )
        for token in BANNED_TOKENS
        if token in clean
    ]


def rust_classes() -> set[str]:
    """Every ``wl-`` token appearing in UI source, split on whitespace.

    Splitting matters: a class attribute is written ``"wl-btn wl-primary"``
    and a naive prefix-only match sees one class instead of two, which
    made this check report live rules as dead.
    """
    found: set[str] = set()
    for path in UI_SRC.rglob("*.rs"):
        text = path.read_text()
        text = re.sub(r"//.*", "", text)
        # Custom-property NAMES are not classes. `var(--wl-text-primary)`
        # and `--wl-accent-coral` both contain a `wl-` token, and a loose
        # scan reports them as markup asking for a rule that cannot exist.
        #
        # The lookbehind is load-bearing: a BEM modifier is ALSO a `--`
        # token, so an unanchored stripper eats the `--done` out of
        # `"wl-task-row--done"` and the linter then reports every modifier
        # class in the file as dead.
        text = re.sub(r"var\(\s*--[a-z0-9-]+\s*\)", " ", text)
        text = re.sub(r"(?<![\w-])--[a-z0-9-]+", " ", text)
        for literal in re.findall(r'"([^"]*)"', text):
            found.update(w for w in literal.split() if w.startswith("wl-"))
        found.update(re.findall(r"\bwl-[a-z0-9-]+\b", text))
    return found


def check_class_coverage(clean: str) -> list[Finding]:
    """5/6. No class without a rule, and no rule without a class."""
    defined = set(re.findall(r"\.([A-Za-z][A-Za-z0-9_-]*)", clean))
    defined = {c for c in defined if c.startswith("wl-")}
    used = rust_classes()
    found = [
        Finding("class-without-rule", "ui/src", f"`.{cls}` is used in markup but has no rule")
        for cls in sorted(used - defined)
    ]
    found += [
        Finding("dead-rule", "wl.css", f"`.{cls}` has a rule but no markup uses it")
        for cls in sorted(defined - used)
    ]
    return found


def baseline_selectors(ref: str) -> set[str]:
    result = subprocess.run(
        ["git", "show", f"{ref}:{CSS.relative_to(REPO)}"],
        capture_output=True,
        text=True,
        cwd=REPO,
    )
    if result.returncode != 0:
        raise SystemExit(f"css-lint: cannot read {ref}:{CSS.relative_to(REPO)}")
    return set(selectors(strip_comments(result.stdout)))


def check_theme_shadowing(current: str, base: set[str]) -> list[Finding]:
    """7. A rule must not have become light-mode-only.

    The check the other six cannot make. ``[data-theme="light"] X`` is a
    perfectly valid selector and may be entirely correct -- but if ``X``
    used to be unconditional, something narrowed it, and the question is
    always "was that deliberate?". A careless deletion of a themed
    override leaves exactly this shape behind.
    """
    found = []
    for sel in sorted(set(current) - base):
        if not sel.startswith('[data-theme="') or " " not in sel:
            continue
        bare = sel.split(" ", 1)[1]
        if bare and bare in base:
            found.append(
                Finding(
                    "themed-shadowed",
                    "wl.css",
                    f"{sel!r} is new, but {bare!r} used to be unconditional. Either the "
                    "themed rule is a deliberate addition, or this is a stray prefix left "
                    "by a deletion -- check whether the base rule should still apply in "
                    "dark mode.",
                )
            )
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--against",
        default="HEAD",
        help="git ref holding the known-good stylesheet ('' to skip)",
    )
    args = parser.parse_args()

    raw = CSS.read_text()
    clean = strip_comments(raw)
    current = selectors(clean)

    findings: list[Finding] = []
    findings += check_tokens(clean)
    findings += check_duplicate_selectors(clean)
    findings += check_dangling_prefix(raw)
    findings += check_banned_tokens(clean)
    findings += check_class_coverage(clean)
    if args.against:
        findings += check_theme_shadowing(current, baseline_selectors(args.against))

    if not findings:
        where = f"vs {args.against}" if args.against else "(no baseline)"
        print(f"css-lint: clean {where}")
        return 0

    for finding in findings:
        print(finding)
    print(f"\ncss-lint: {len(findings)} finding(s)")
    return 1


if __name__ == "__main__":
    sys.exit(main())
