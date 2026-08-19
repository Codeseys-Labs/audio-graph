#!/usr/bin/env python3
"""Machine-check every ``file:line`` anchor claimed by the audio-graph-68a1 docs.

What this script guarantees, exactly:

1. It enumerates every ``<path>:<line>`` anchor in the SCANNED regions listed in
   ``SCANNED`` below and asserts that the cited line of the cited file contains
   the substring recorded for it in ``EXPECTED``.
2. It FAILS on any anchor it finds that ``EXPECTED`` does not enumerate, so an
   anchor cannot be added to a doc without being checked here.
3. It FAILS on any ``EXPECTED`` entry no scanned region cites, so the table
   cannot rot into a set of claims nobody makes.
4. It FAILS on a bare ``:1234``-style anchor anywhere in a scanned region. The
   68a1 docs cite ``path:line``, never a bare line number, because a bare number
   silently detaches from the file it was written against.

What it deliberately does NOT check:

* Fenced code blocks are stripped before scanning. Verbatim tool output is
  reproduced in fenced blocks, and the ``src/persistence/...:NNNN:CC`` locations
  a panic prints refer to the tree the panic happened in, not to this tree; they
  are evidence, not claims.
* ``docs/commit-state-2026-08-16-session-control-contract-wave7c.md`` and
  ``docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md``
  are scanned ONLY between their ``audio-graph-68a1 anchors`` markers. Their
  older rounds carry anchors from earlier commits that those documents already
  declare unmaintained; this run does not adopt them.

Usage: ``python3 docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py``
Exit 0 means every claim above held.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
RUN_DIR = "docs/agentic-runs/2026-08-18-audio-graph-68a1"
WAVE7C = "docs/commit-state-2026-08-16-session-control-contract-wave7c.md"
REPORT_3B53 = "docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md"
REPORT_68A1_POINTER = (
    "docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-68a1-report.md"
)

# Production files that must carry no line anchors at all (see the check below).
SOURCE_FILES_WITHOUT_ANCHORS: list[str] = [
    "src-tauri/src/persistence/session_artifact_manifest.rs",
    "src-tauri/src/persistence/canonical_durability.rs",
]

MARKER_BEGIN = "<!-- audio-graph-68a1 anchors: begin -->"
MARKER_END = "<!-- audio-graph-68a1 anchors: end -->"

# (document, whole-file or marker-delimited)
SCANNED: list[tuple[str, bool]] = [
    (f"{RUN_DIR}/plan.md", False),
    (f"{RUN_DIR}/report.md", False),
    (REPORT_68A1_POINTER, False),
    (WAVE7C, True),
    (REPORT_3B53, True),
]

MANIFEST = "src-tauri/src/persistence/session_artifact_manifest.rs"
SEMANTICS = "src-tauri/src/persistence/session_semantics.rs"
LOG = "src-tauri/src/persistence/canonical_log.rs"
ADR = "docs/adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md"

EXPECTED: dict[tuple[str, int], str] = {
    # Shipped by this seed.
    (MANIFEST, 442): "pub enum V2ProvenanceProofBindingError {",
    (MANIFEST, 673): "V2ProvenanceProofBinding(V2ProvenanceProofBindingError),",
    (MANIFEST, 1631): "fn refuse_unproven_v2_candidate(",
    (MANIFEST, 1697): "self.refuse_unproven_v2_candidate(&candidate, proof_owned)",
    (MANIFEST, 2324): "pub(crate) struct V2SessionProvenanceEntry<",
    (MANIFEST, 2329): "pub(crate) fn validate_v2_session_provenance(",
    (MANIFEST, 2357): "if manifest.transition.fingerprint != content.sha256 {",
    (MANIFEST, 2374): "const MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES: u64 = 4096;",
    (MANIFEST, 2392): "fn bind_v2_provenance_to_durable_proof(",
    (MANIFEST, 2444): "fn binding_io(error: io::Error) -> V2ProvenanceProofBindingError {",
    # Pre-existing production code this seed reasons about.
    (MANIFEST, 256): "session_semantics_version: SessionSemanticsVersion::V1,",
    (MANIFEST, 1346): "self.prepare_compare_and_swap(expected_generation, candidate, true)",
    (MANIFEST, 1356): ".preflight_immutable_exact(provenance_path, &proof_bytes, self.qualification)",
    (MANIFEST, 1730): "return reject(ManifestCasRejection::IdempotencyConflict);",
    (MANIFEST, 1743): "return reject(ManifestCasRejection::TransitionConflict);",
    (MANIFEST, 1749): "return reject(ManifestCasRejection::CompletionRequiresPrepared);",
    (MANIFEST, 1781): "let recovery_key = recovery_key(&candidate.transition.fingerprint);",
    (MANIFEST, 1789): "fn commit_prepared_compare_and_swap(",
    (MANIFEST, 1989): "fn load_manifest_file(path: &Path)",
    (MANIFEST, 2005): "let opened_metadata = file.metadata().map_err(|error| load_io(&error))?;",
    # Tests.
    (MANIFEST, 3722): "fn v2_candidate_requires_exact_bound_session_provenance_proof() {",
    (MANIFEST, 3866): "fn accepted_v2_manifest_cannot_regress_to_v1() {",
    (MANIFEST, 3894): "fn proof_before_manifest_transition_returns_actual_accepted_and_already_completed() {",
    (MANIFEST, 3967): "fn addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes() {",
    (MANIFEST, 4146): "fn committed_v2_head_records_later_generations_only_against_the_durable_proof() {",
    (MANIFEST, 4216): "fn advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts() {",
    (MANIFEST, 4273): "fn quarantine_recovery_remains_closed_on_a_v2_session() {",
    (MANIFEST, 4356): "fn forged_v2_provenance_is_refused_on_an_advanced_head() {",
    (MANIFEST, 4465): "fn advanced_head_refuses_later_generations_when_the_durable_proof_is_not_intact() {",
    (MANIFEST, 4562): "fn proof_conflict_and_indeterminate_prevent_manifest_mutation_then_retry_converges() {",
    (MANIFEST, 4647): "fn stale_existing_head_rejects_different_transition_before_proof_mutation() {",
    # Other modules.
    (SEMANTICS, 113): "pub fn checked_session_open<T, E>(",
    (SEMANTICS, 167): "pub fn admitted_session_semantics_floor(",
    (SEMANTICS, 194): "validate_v2_session_provenance(manifest)",
    (LOG, 349): "Manifest(ManifestCasRejection),",
    (LOG, 973): "fn manifest_candidate(",
    (LOG, 1032): "SessionArtifactManifestV1::candidate(",
    (LOG, 1107): ".compare_and_swap_recovery(self.expected_generation, candidate)",
    # ADR-0038, the authority this seed closes an implementation gap against.
    (ADR, 2): "status: accepted",
    (ADR, 158): "A mutator holds the exclusive guard across",
    (ADR, 188): "A missing, duplicate, altered, unavailable,",
}

ANCHOR = re.compile(r"(?P<file>[A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:rs|md|py|toml)):(?P<line>\d+)")
BARE_ANCHOR = re.compile(r"(?<![\w]):\d{2,}")
FENCE = re.compile(r"^\s*```")


def scannable(text: str) -> str:
    """Drop fenced blocks: they hold reproduced tool output, not claims."""
    kept, inside = [], False
    for line in text.splitlines():
        if FENCE.match(line):
            inside = not inside
            continue
        kept.append("" if inside else line)
    return "\n".join(kept)


def region(text: str, doc: str) -> str:
    if MARKER_BEGIN not in text or MARKER_END not in text:
        raise SystemExit(f"FAIL {doc}: missing the audio-graph-68a1 anchor markers")
    body = text.split(MARKER_BEGIN, 1)[1].split(MARKER_END, 1)[0]
    return body


def main() -> int:
    failures: list[str] = []
    cited: set[tuple[str, int]] = set()

    for doc, marker_delimited in SCANNED:
        path = REPO_ROOT / doc
        if not path.is_file():
            failures.append(f"FAIL missing scanned document: {doc}")
            continue
        text = path.read_text(encoding="utf-8")
        if marker_delimited:
            text = region(text, doc)
        text = scannable(text)

        for match in BARE_ANCHOR.finditer(ANCHOR.sub("ANCHOR", text)):
            failures.append(
                f"FAIL {doc}: bare line anchor {match.group(0)!r}; cite path:line instead"
            )

        for match in ANCHOR.finditer(text):
            file_path, line_no = match.group("file"), int(match.group("line"))
            key = (file_path, line_no)
            cited.add(key)
            expected = EXPECTED.get(key)
            if expected is None:
                failures.append(f"FAIL {doc}: anchor {file_path}:{line_no} is not enumerated here")
                continue
            target = REPO_ROOT / file_path
            if not target.is_file():
                failures.append(f"FAIL {doc}: anchor {file_path}:{line_no} names no such file")
                continue
            lines = target.read_text(encoding="utf-8").splitlines()
            if line_no < 1 or line_no > len(lines):
                failures.append(
                    f"FAIL {file_path}:{line_no} is out of range (file has {len(lines)} lines)"
                )
                continue
            actual = lines[line_no - 1]
            if expected not in actual:
                failures.append(
                    f"FAIL {file_path}:{line_no} expected {expected!r}, found {actual.strip()!r}"
                )

    for key in sorted(EXPECTED.keys() - cited):
        failures.append(f"FAIL enumerated anchor {key[0]}:{key[1]} is cited by no scanned document")

    # A line anchor inside a source comment cannot be checked the way a doc's can
    # (the anchor and its target move together, and this script scans documents),
    # so the invariant is that the production files carry NONE. Review of this
    # branch found the one pre-existing `(:1544)` rotted by +119 inserted lines
    # while this script reported OK, because it never reads .rs files.
    source_anchor = re.compile(r"\(:\d+\)|`:\d+`|[A-Za-z_]+\.rs:\d+")
    for rel in SOURCE_FILES_WITHOUT_ANCHORS:
        target = REPO_ROOT / rel
        for number, text in enumerate(target.read_text(encoding="utf-8").splitlines(), 1):
            if source_anchor.search(text):
                failures.append(
                    f"FAIL {rel}:{number} carries a line anchor in source; "
                    f"cite the symbol name instead: {text.strip()!r}"
                )

    if failures:
        print("\n".join(failures))
        print(f"{len(failures)} anchor failure(s); {len(cited)} anchor(s) enumerated and cited")
        return 1

    print(f"OK {len(cited)} anchors checked across {len(SCANNED)} documents; 0 failures")
    return 0


if __name__ == "__main__":
    sys.exit(main())
