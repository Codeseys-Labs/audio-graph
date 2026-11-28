/**
 * Notes synthesis controller (`synthesize_notes` over the backend's CURRENT
 * buffers), split out of NotesPanel so that a Review pane, which must never
 * call it, does not even mount it: `NotesPanel` calls `useNotesSynthesis` only
 * from `OwnSynthesisNotesPanel`, which is rendered outside a Review provider.
 */
// `safeInvoke` (aliased to `invoke`) is a drop-in for the Tauri `invoke` that
// relays a command-name-only failure diagnostic to analytics then rethrows, so
// this call site's error handling is unchanged (audio-graph-3e71).
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import { useSessionView } from "../session/SessionViewProvider";
import { useActiveGraphSnapshot } from "../session/useActiveGraphSnapshot";
import { deferredProviderForLlmStart, useAudioGraphStore } from "../store";
import { errorToMessage } from "../utils/errorToMessage";

/**
 * A point-in-time fingerprint of the inputs to a synthesis run. We capture it
 * when synthesis succeeds, then compare against the live store to surface a
 * lightweight "may be out of date" hint when the graph or transcript grows.
 */
export interface SynthesisResult {
  markdown: string;
  at: number;
  nodeCount: number;
  segmentCount: number;
}

/**
 * Manual whole-session synthesis, extracted from NotesPanel's body (SHELL-R2,
 * plan §R2, ADR-0046) so its trigger can live somewhere other than NotesPanel's own
 * header without forking the state two renderers would otherwise disagree
 * about. In the Capture/During workspace NotesPanel calls this itself
 * (uncontrolled — see the `synthesis` prop below); in the Sessions detail's
 * Notes lens, the detail chrome that owns the "Generate prose summary"
 * overflow item calls it and hands the SAME controller instance to
 * `<NotesPanel synthesis={...} headerActions={false} />` so the result/error
 * NotesPanel renders is the run the overflow item actually triggered.
 */
export interface NotesSynthesisController {
  loading: boolean;
  error: string | null;
  result: SynthesisResult | null;
  handleSynthesize: () => Promise<void>;
  clearError: () => void;
}

export function useNotesSynthesis(): NotesSynthesisController {
  const { t } = useTranslation();
  const { transcriptSegments: segments } = useSessionView();
  // The merged snapshot (materialized graph when the Graph lens has loaded
  // one, live graph otherwise) — the same seam `KnowledgeGraphViewer` reads,
  // so the "is this synthesis stale" node-count comparison below agrees with
  // what the Notes lens's own entity chips (`NotesPanel`'s main body, below)
  // are actually showing (fix-round finding: reading raw `graphSnapshot`
  // here silently degraded to the live graph's `MAX_NODES`/`MAX_EDGES`-capped,
  // retraction-unaware view for any historical session).
  const { snapshot: graph } = useActiveGraphSnapshot();
  const settings = useAudioGraphStore((s) => s.settings);
  const loadedSessionId = useAudioGraphStore((s) => s.loadedSessionId);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SynthesisResult | null>(null);

  const handleSynthesize = useCallback(async () => {
    if (loading) return;
    if (loadedSessionId) {
      setError(t("notes.reviewSynthesisBlocked"));
      return;
    }
    if (!settings) {
      setError(t("errors.providerSettingsLoading"));
      return;
    }
    const deferredProvider = deferredProviderForLlmStart(settings);
    if (deferredProvider) {
      setError(
        errorToMessage({
          code: "provider_deferred",
          message: {
            provider_id: deferredProvider.id,
            display_name: deferredProvider.display_name,
          },
        }),
      );
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const markdown = await invoke<string>("synthesize_notes");
      setResult({
        markdown,
        at: Date.now(),
        nodeCount: graph.nodes?.length ?? 0,
        segmentCount: segments.length,
      });
    } catch (e) {
      setError(errorToMessage(e));
    } finally {
      setLoading(false);
    }
  }, [graph, loadedSessionId, loading, segments, settings, t]);

  return {
    loading,
    error,
    result,
    handleSynthesize,
    clearError: () => setError(null),
  };
}
