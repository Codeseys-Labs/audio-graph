# Session Management Design

**Date:** 2026-04-16
**Status:** Design proposal

## Current State

Sessions auto-save per-UUID to `~/.audiograph/`:
- `transcripts/<uuid>.jsonl` — JSONL transcript segments (0600 perms)
- `graphs/<uuid>.json` — pretty-printed graph (0600 perms, 30s auto-save)

**Session ID:** UUID v4 generated fresh on every app launch.
No cross-launch continuity. No metadata. No UI to browse.

**Backend has:**
- `get_session_id()` → current UUID
- `load_graph(path)` → load a graph file
- `save_graph()` / `export_graph()` → write graph
- `export_transcript()` → in-memory buffer as JSON string

**Missing:**
- `list_sessions()`, `load_transcript(id)`, session metadata
- Frontend UI for any of the above

## Critical Gaps

1. **Session loss on crash** — 30s window between graph autosaves
2. **No session metadata** — can't see creation time, duration, stats
3. **No continuity across restarts** — old sessions orphaned on disk
4. **No UI discoverability** — files exist but user can't find them

## Proposed Solution

### v1 Minimal (1 day effort)

**Backend:**
1. New file: `~/.audiograph/sessions.json` — lightweight metadata index
2. Schema:
   ```json
   [{
     "id": "uuid",
     "title": null,
     "created_at": "2026-04-16T12:00:00Z",
     "ended_at": "2026-04-16T13:30:00Z",
     "duration_seconds": 5400,
     "status": "active" | "complete" | "crashed",
     "segment_count": 247,
     "speaker_count": 3,
     "entity_count": 89,
     "transcript_path": "transcripts/uuid.jsonl",
     "graph_path": "graphs/uuid.json"
   }]
   ```
3. New Tauri commands:
   - `list_sessions(limit: usize) -> Vec<SessionMetadata>`
   - `load_transcript(id: String) -> Vec<TranscriptSegment>`
   - `load_session(id: String) -> SessionSnapshot` (transcript + graph)
   - `delete_session(id: String)`
4. Lifecycle:
   - App start: create entry, status="active"
   - Every 30s: update segment_count + entity_count stats
   - App shutdown: mark "complete", set ended_at, duration

**Frontend:**
- "Sessions" button in ControlBar → opens `SessionsBrowser` modal
- Shows 10 most recent sessions with title/date/duration/stats
- Click → load transcript + graph into current view
- "Load Last Session" quick button on startup if prior session exists

### v2 Full Browser (2-3 days)

- Rename sessions with custom titles
- Search by date range, speaker, entity
- Delete with confirmation
- Bulk export (ZIP with transcript + graph)
- Tag sessions

## Design Decisions

1. **JSON index not SQLite** — simpler, human-readable, no dep, fast for <1000 sessions
2. **UUID-based naming** — collision-free, privacy-friendly
3. **0600 file perms** — transcripts may contain PII
4. **JSONL format** — line-based, atomic, resilient to partial writes

## Edge Cases

| Case | Handling |
|------|----------|
| Crash mid-session | Next start sees "active" status → offer recovery |
| Partial JSONL write | Line-based format skips incomplete lines |
| Missing files | Graceful error, offer to purge orphaned index entry |
| Index corruption | Fall back to empty, scan disk to rebuild |
| Concurrent sessions | UUID naming prevents collisions |

## Data Flow (v1)

```
App Startup:
  generate UUID
  append SessionMetadata{status: "active", created_at: now} to sessions.json

During Session:
  every 30s: update segment_count, entity_count, duration in sessions.json

Load Prior Session:
  user clicks Sessions → list_sessions()
  user clicks entry → load_session(id) → populate transcript + graph

App Close:
  mark status = "complete", set ended_at, final duration

Next Start:
  if last session has status == "active" → offer recovery
  else normal start
```
