# audio-graph-68a1 — provenance-to-durable-proof binding (wave index pointer)

The canonical documents for seed `audio-graph-68a1` live in the dated run
directory, not here:

- design (the brief as amended by the adversarial critique):
  `docs/agentic-runs/2026-08-18-audio-graph-68a1/plan.md`
- report (what shipped, verbatim RED and gate output, residuals with owners):
  `docs/agentic-runs/2026-08-18-audio-graph-68a1/report.md`
- anchor check that every `path:line` in both of those is machine-verified
  against this tree: `docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py`

This file exists so the wave-7c index has an entry at the path
`audio-graph-3b53-report.md` and the wave-7c commit-state document point at. It
duplicates no content.

What 68a1 closed, in one paragraph: a V2 candidate's `SessionProvenanceEvents`
entry is now provably tied to the Session's durable v1-to-v2 transition-proof
record, not merely self-consistent, and only then was the transition-proof gate
re-keyed from the candidate's floor onto whether this call performs the
transition. audio-graph-3b53 finding B4 is closed by that re-key; the pinning test
that held the wedge open was inverted, not deleted. Quarantine recovery on a V2
Session is still closed for every candidate shape, and that closure now has
executable evidence.
