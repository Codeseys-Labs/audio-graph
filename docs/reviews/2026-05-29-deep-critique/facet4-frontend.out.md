SUCCESS: The process with PID 213532 (child process of PID 214024) has been terminated.
SUCCESS: The process with PID 214024 (child process of PID 216000) has been terminated.
SUCCESS: The process with PID 216000 (child process of PID 214756) has been terminated.
SUCCESS: The process with PID 214756 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 216300 (child process of PID 216184) has been terminated.
SUCCESS: The process with PID 216184 (child process of PID 216068) has been terminated.
SUCCESS: The process with PID 216068 (child process of PID 215408) has been terminated.
SUCCESS: The process with PID 215408 (child process of PID 215464) has been terminated.
SUCCESS: The process with PID 215464 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 215904 (child process of PID 215840) has been terminated.
SUCCESS: The process with PID 215840 (child process of PID 215792) has been terminated.
SUCCESS: The process with PID 215792 (child process of PID 213612) has been terminated.
SUCCESS: The process with PID 213612 (child process of PID 214796) has been terminated.
SUCCESS: The process with PID 214796 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 216468 (child process of PID 211004) has been terminated.
SUCCESS: The process with PID 211004 (child process of PID 216780) has been terminated.
SUCCESS: The process with PID 216780 (child process of PID 215760) has been terminated.
SUCCESS: The process with PID 215760 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 215864 (child process of PID 215832) has been terminated.
SUCCESS: The process with PID 215832 (child process of PID 215780) has been terminated.
SUCCESS: The process with PID 215780 (child process of PID 206220) has been terminated.
SUCCESS: The process with PID 206220 (child process of PID 214840) has been terminated.
SUCCESS: The process with PID 214840 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 216428 (child process of PID 216272) has been terminated.
SUCCESS: The process with PID 216272 (child process of PID 216092) has been terminated.
SUCCESS: The process with PID 216092 (child process of PID 213192) has been terminated.
SUCCESS: The process with PID 213192 (child process of PID 215604) has been terminated.
SUCCESS: The process with PID 215604 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 216076 (child process of PID 215984) has been terminated.
SUCCESS: The process with PID 215984 (child process of PID 215988) has been terminated.
SUCCESS: The process with PID 215988 (child process of PID 201128) has been terminated.
SUCCESS: The process with PID 201128 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 208992 (child process of PID 215352) has been terminated.
SUCCESS: The process with PID 215352 (child process of PID 212296) has been terminated.
SUCCESS: The process with PID 205444 (child process of PID 214908) has been terminated.
SUCCESS: The process with PID 214908 (child process of PID 212296) has been terminated.
CONFIRMED GOOD

- Most components use Zustand selectors rather than whole-store subscription, e.g. `src/App.tsx:95-105`, `src/components/KnowledgeGraphViewer.tsx:35-37`, `src/components/LiveTranscript.tsx:41-45`.
- Transcript and Gemini transcript buffers are capped at 500 items (`src/store/index.ts:173-177`, `src/store/index.ts:520-523`); backend graph cap is 1000 nodes / 5000 edges (`src-tauri/src/graph/temporal.rs:31-35`).
- `ResizeObserver` cleanup is present (`src/components/KnowledgeGraphViewer.tsx:67-82`).
- Modals use `role="dialog"`, `aria-modal`, labels, and focus trap (`src/components/SettingsPage.tsx:108-110`, `src/components/SessionsBrowser.tsx:140-142`, `src/components/ShortcutsHelpModal.tsx:38-65`; trap cleanup in `src/hooks/useFocusTrap.ts:92-101`).
- Rust IPC shapes generally mirror TS for tagged/snake_case events (`src/types/index.ts:76-119`, `src-tauri/src/events.rs:132-225`; Gemini in `src/types/index.ts` vs `src-tauri/src/gemini/mod.rs:77-165`).

ISSUES

- MED: High-frequency events besides chat are not throttled. `transcript-update`, `asr-partial`, `graph-delta`, and `pipeline-latency` write directly to Zustand (`src/hooks/useTauriEvents.ts:252-320`; setters at `src/store/index.ts:173-178`, `322-375`, `397-405`). Chat deltas are coalesced at ~30fps (`src/hooks/useTauriEvents.ts:198-229`), but these other floods can still drive `LiveTranscript`, `KnowledgeGraphViewer`, and `PipelineStatusBar` every event.
- MED: Async listener cleanup has an unmount race. Cleanup only iterates the current `unlisten` array (`src/hooks/useTauriEvents.ts:187-188`, `250-386`, `388-395`); if the component unmounts before `Promise.all` resolves, later-resolved listeners are installed but never unlistened.
- LOW: `SettingsPage` and `ExpressSetup` subscribe to the whole store (`src/components/SettingsPage.tsx:111-124`, `src/components/ExpressSetup.tsx:60`), so while mounted they re-render on every unrelated high-rate event. Split these into selectors.
- LOW: Graph simulation reheats only when node count increases (`src/components/KnowledgeGraphViewer.tsx:84-112`). Replacing a graph with different nodes at the same/lower count, or add+evict deltas, can seed new nodes but not reheat enough to settle.
- LOW: Graph tooltips interpolate unescaped backend/model-derived text as HTML (`src/components/KnowledgeGraphViewer.tsx:273-284`), risking broken tooltip markup or injection-like rendering from entity names/descriptions.

QUESTIONS

- Is comparison mode intentional? The UI explicitly allows Transcribe + Gemini simultaneously (`src/components/ControlBar.tsx:121-132`, `189-212`), while `nativeS2sEnabled` is only a localStorage UI gate (`src/store/index.ts:572-589`). If modes should be exclusive, this is a state-desync footgun.
SUCCESS: The process with PID 216124 (child process of PID 211560) has been terminated.
SUCCESS: The process with PID 211560 (child process of PID 216892) has been terminated.
SUCCESS: The process with PID 216892 (child process of PID 215696) has been terminated.
SUCCESS: The process with PID 215696 (child process of PID 212296) has been terminated.
