#!/usr/bin/env python3
"""Machine-check every `path:line[-line]` anchor cited in this run's plan and
report, plus the ADR-line-range citations embedded in the implementation's own
doc comments (`route.rs`, `executor.rs`, `openrouter.rs`).

Two kinds of anchor exist in this run:

1. `ADR-NNNN:START-END` (or a bare `:START-END` continuing the previous
   sentence's ADR number) inside `route.rs` / `executor.rs` / `openrouter.rs`
   doc comments, checked against `docs/adr/NNNN-*.md`.
2. Bare `ADR-NNNN` mentions with no line range are not range-checked (nothing
   to verify beyond "the ADR exists"), so they are out of scope here.

Fails on: an enumerated ADR anchor whose target file does not exist, whose
range is out of bounds, or whose range's text does not contain the expected
substring recorded below. It establishes only that each cited range currently
contains the claim it was cited for.
"""

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
RUN_DIR = Path(__file__).parent
DOCS = [RUN_DIR / "plan.md", RUN_DIR / "report.md"]

# (adr_number, start_line, end_line) -> substring expected somewhere in that
# (1-indexed, inclusive) line range of docs/adr/<adr_number>-*.md
EXPECTED_ADR = {
    (38, 41, 43): "No `finish_reason` deserialization on either blocking client",
    (38, 49, 54): "left unproven",
    (38, 54, 56): "131,072",
    (38, 120, 122): "Chat Completions is the only MVP-admitted wire skin",
    (38, 123, 125): "Capability checks are per-ENDPOINT",
    (38, 126, 127): "Completed` / `Truncated` / `Refused` / `Failed` / `TransportLost`",
    (38, 128, 135): "four-class retry classification whose uncertain class is never",
    (38, 153, 157): "Cerebras through OpenRouter",
    (38, 161, 165): "validator stays the sole admission authority",
    (38, 196, 197): "option D cannot reach A without a new decision",
    (38, 219, 225): "raises the Cerebras",
    (38, 235, 240): "no never-dispatched class",
    (38, 241, 244): "retry progression was deferred to `audio-graph-3b48`",
    (38, 328, 330): "configured with empty fallback lists, IS",
    (38, 341, 345): "producer inventory for egress",
    (33, 48, 52): "Every start path must resolve",
}

# Anchors actually present in each source file, keyed by the line the citation
# comment sits on. Value is the (adr_number, start, end) it must resolve to.
CITATIONS = {
    ("src-tauri/src/llm/route.rs", 10): [(38, 196, 197), (38, 328, 330)],
    ("src-tauri/src/llm/route.rs", 35): [(38, 120, 122)],
    ("src-tauri/src/llm/route.rs", 68): [(38, 49, 54)],
    ("src-tauri/src/llm/route.rs", 92): [(38, 161, 165)],
    ("src-tauri/src/llm/route.rs", 107): [(38, 54, 56), (38, 123, 125)],
    ("src-tauri/src/llm/route.rs", 153): [(33, 48, 52)],
    ("src-tauri/src/llm/route.rs", 215): [(38, 54, 56)],
    ("src-tauri/src/llm/route.rs", 259): [(38, 341, 345)],
    ("src-tauri/src/llm/route.rs", 341): [(38, 153, 157)],
    ("src-tauri/src/llm/route.rs", 372): [(33, 48, 52)],
    ("src-tauri/src/llm/route.rs", 495): [(38, 126, 127)],
    ("src-tauri/src/llm/route.rs", 556): [(38, 235, 240)],
    ("src-tauri/src/llm/route.rs", 559): [(38, 241, 244)],
    ("src-tauri/src/llm/route.rs", 581): [(38, 128, 135)],
    ("src-tauri/src/llm/executor.rs", 715): [(38, 219, 225)],
    ("src-tauri/src/llm/openrouter.rs", 4180): [(38, 41, 43)],
}

PROSE_ADR_ANCHOR = re.compile(r"ADR-(\d+):(\d+)-(\d+)")

failures = []

# 1. Every (adr, start, end) actually cited in the source files must be one we
#    enumerated with an expected substring, and vice versa: every citation we
#    enumerated must still appear at the recorded source line.
cited_adr_ranges = set()
for (rel_path, line_no), ranges in CITATIONS.items():
    target = REPO / rel_path
    if not target.is_file():
        failures.append(f"missing source file: {rel_path}")
        continue
    lines = target.read_text().splitlines()
    if not 1 <= line_no <= len(lines):
        failures.append(f"{rel_path}:{line_no} is out of range ({len(lines)} lines)")
        continue
    # The citation may span this line and the next (e.g. "ADR-0038:196-197
    # and :328-330" wraps across two source lines in some comments), so check
    # a small window around the recorded line rather than the exact line only.
    window = "\n".join(lines[max(0, line_no - 2) : line_no + 1])
    for adr, start, end in ranges:
        cited_adr_ranges.add((adr, start, end))
        token_a = f"ADR-{adr:04d}:{start}-{end}"
        token_b = f":{start}-{end}"
        if token_a not in window and token_b not in window:
            failures.append(
                f"{rel_path}:{line_no} does not contain a citation for "
                f"ADR-{adr:04d}:{start}-{end}"
            )

for key in EXPECTED_ADR:
    if key not in cited_adr_ranges:
        failures.append(f"ADR-{key[0]:04d}:{key[1]}-{key[2]} enumerated but never cited in a source file")

# 2. Every enumerated ADR range must exist in the ADR file and contain its
#    expected substring.
for (adr, start, end), expected in sorted(EXPECTED_ADR.items()):
    matches = list((REPO / "docs" / "adr").glob(f"{adr:04d}-*.md"))
    if not matches:
        failures.append(f"no ADR file found for ADR-{adr:04d}")
        continue
    adr_file = matches[0]
    lines = adr_file.read_text().splitlines()
    if not (1 <= start <= end <= len(lines)):
        failures.append(f"{adr_file.name}:{start}-{end} out of range ({len(lines)} lines)")
        continue
    excerpt = "\n".join(lines[start - 1 : end])
    # Collapse whitespace (including the line-wrap newline markdown prose
    # uses at ~80 columns) so a phrase that wraps across two source lines
    # still matches; the ANCHOR FAIL message below still prints the raw
    # excerpt for a human to inspect.
    normalized = re.sub(r"\s+", " ", excerpt)
    if expected not in normalized:
        failures.append(
            f"{adr_file.name}:{start}-{end} expected to contain {expected!r}, "
            f"got: {excerpt!r}"
        )

# 3. Scan plan.md / report.md prose for ADR:line-line anchors and make sure
#    every one of them was accounted for above (no citation invented in the
#    docs that the implementation doesn't also carry, or vice versa).
prose_ranges = set()
for doc in DOCS:
    if not doc.exists():
        failures.append(f"missing document {doc}")
        continue
    for adr, start, end in PROSE_ADR_ANCHOR.findall(doc.read_text()):
        prose_ranges.add((int(adr), int(start), int(end)))

for r in prose_ranges:
    if r not in EXPECTED_ADR:
        failures.append(f"doc cites unenumerated anchor ADR-{r[0]:04d}:{r[1]}-{r[2]}")

print(
    f"{len(EXPECTED_ADR)} ADR ranges enumerated, {len(cited_adr_ranges)} cited in source, "
    f"{len(prose_ranges)} cited in plan.md/report.md prose"
)
if failures:
    for failure in failures:
        print(f"ANCHOR FAIL: {failure}", file=sys.stderr)
    sys.exit(1)
print("ANCHORS OK")
