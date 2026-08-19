#!/usr/bin/env python3
"""Machine-check every `path:line` anchor cited in this run's plan and report.

Fails on: an anchor extracted from prose that is not enumerated below, an
enumerated file that does not exist, an out-of-range line, or a line whose text
does not contain the expected substring. It establishes only that each cited
line currently contains the named symbol or code.
"""

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
DOCS = [Path(__file__).with_name("plan.md"), Path(__file__).with_name("report.md")]

EXPECTED = {
    ("src-tauri/src/persistence/session_semantics.rs", 167): "pub fn admitted_session_semantics_floor(",
    ("src-tauri/src/persistence/session_semantics.rs", 200): "if current == accepted {",
    ("src-tauri/src/persistence/session_semantics.rs", 249): "pub fn admit_session_semantics_v1_to_v2(",
    ("src-tauri/src/persistence/session_semantics.rs", 402): "pub fn guarded_session_open<T, E>(",
    ("src-tauri/src/persistence/session_semantics.rs", 486): "fn unguarded_absence_admission<T, E>(",
    ("src-tauri/src/persistence/session_semantics.rs", 526): "pub fn open_session_for_content<T, E>(",
    ("src-tauri/src/persistence/session_semantics.rs", 996): 'HISTORICAL_ORIGINAL_AUDIO_IDENTITY: &str = "audio/original-session-audio"',
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 152): "HistoricalUnknown,",
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 1576): "pub fn abandon_staged_transition(&self)",
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 2175): "MissingOriginalSessionAudio",
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 2676): "fn is_internal_identity(identity: &str) -> bool {",
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 2890): "pub fn retire_owned_control_plane(",
    ("src-tauri/src/persistence/session_artifact_manifest.rs", 5350): "fn windows_other_session_transition_refuses_before_any_control_mutation()",
    ("src-tauri/src/persistence/canonical_durability.rs", 1388): "pub(crate) fn unlink_canonical_entry(",
    ("src-tauri/src/persistence/canonical_durability.rs", 3740): "const fn namespace_supported_for(platform: CanonicalPlatform) -> bool {",
    ("src-tauri/src/sessions/mod.rs", 1679): "assert_eq!(actual.len(), 18",
    ("src-tauri/src/commands.rs", 6911): "fn read_session_transcript_snapshot(",
}

ANCHOR = re.compile(r"(src-tauri/src/[A-Za-z0-9_./-]+\.rs):(\d+)")

failures = []
extracted = set()
for doc in DOCS:
    if not doc.exists():
        failures.append(f"missing document {doc}")
        continue
    for path, line in ANCHOR.findall(doc.read_text()):
        anchor = (path, int(line))
        extracted.add(anchor)
        if anchor not in EXPECTED:
            failures.append(f"{doc.name} cites unenumerated anchor {path}:{line}")

for (path, line), expected in sorted(EXPECTED.items()):
    target = REPO / path
    if not target.is_file():
        failures.append(f"enumerated file does not exist: {path}")
        continue
    lines = target.read_text().splitlines()
    if not 1 <= line <= len(lines):
        failures.append(f"{path}:{line} is out of range (file has {len(lines)} lines)")
        continue
    actual = lines[line - 1]
    if expected not in actual:
        failures.append(f"{path}:{line} expected {expected!r}, found {actual.strip()!r}")

print(
    f"{len(EXPECTED)} anchors enumerated, {len(extracted)} extracted from prose, "
    f"{len(EXPECTED) - len([f for f in failures if 'expected' in f or 'out of range' in f or 'does not exist' in f])} verified"
)
if failures:
    for failure in failures:
        print(f"ANCHOR FAIL: {failure}", file=sys.stderr)
    sys.exit(1)
print("ANCHORS OK")
