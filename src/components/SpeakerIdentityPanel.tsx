/**
 * Speaker identity panel — lazy speaker identification UI (#340 backend,
 * PR #488): the per-session control surface for the backend's durable
 * speaker-identity ledger.
 *
 * Lists the ACTIVE session's speaker clusters (`list_session_speaker_clusters`)
 * and offers the four manual identity edits the backend commands accept — all
 * camelCase IPC args (`{ sessionId, cluster, ... }`), all records snake_case
 * (`ClusterRef.{ provider, timeline_id, speaker_id }`):
 *   - Rename            → `relabel_speaker_cluster(sessionId, cluster, label)`
 *   - This is me        → `mark_cluster_is_self(sessionId, cluster)` (the
 *                         backend reads the configured `self_display_name`
 *                         AppSettings field and rejects with
 *                         "set your display name first" when unset)
 *   - Same as…          → `merge_speaker_clusters(sessionId, cluster, into)`
 *   - Not this person   → `unbind_speaker_cluster(sessionId, cluster)`,
 *                         shown only while a cluster is already bound
 *                         (`bound` / `user_confirmed`).
 *
 * React is display and control ONLY (AGENTS.md architecture guardrail):
 * every mutation goes through the backend command, then this panel re-reads
 * `list_session_speaker_clusters` — local state never becomes the label
 * source of truth. Beyond the post-mutation re-read, the cluster list also
 * refreshes on the speaker/graph revision events the backend emits
 * (`diarization-span-revision`, `graph-update`, `graph-delta`), so clusters
 * other surfaces (timeline, transcript) rename underneath us stay in sync.
 *
 * Awaiting an in-flight IPC mutation disables every action button (one
 * mutation at a time — the backend's relabel batches are atomic), success
 * surfaces in a `role="status"` region and a backend rejection in a
 * `role="alert"` region, both polite live-region-compatible.
 *
 * Session source: the store's `getSessionId()` (backend `get_session_id`).
 * While there is no active session the panel renders a quiet empty state and
 * never issues cluster commands. The session id is re-read on every refresh,
 * so a capture stop/start rollover is picked up by the next event/mutation.
 *
 * Store bindings: `getSessionId`, `isCapturing` (refresh trigger on
 * start/stop). No props.
 *
 * Parents: `App.tsx`'s left aside (below `SpeakerPanel`, pinned at the `wide`
 * tier) and `App.tsx`'s speakers `ShellDrawer` (the same regions
 * `SpeakerPanel` occupies at every tier).
 */
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
// `safeInvoke` (aliased to `invoke`) is a drop-in for the Tauri `invoke` that
// relays a command-name-only failure diagnostic to analytics then rethrows, so
// this call site's error handling is unchanged (audio-graph-3e71).
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import { useAudioGraphStore } from "../store";
import type { ClusterRef, SessionSpeakerCluster } from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import Button from "./Button";

/** Event names — must match `src-tauri/src/events.rs` (same literals as
 * `useTauriEvents.ts`; kept local so this panel does not import the hook's
 * private module scope). */
const DIARIZATION_SPAN_REVISION = "diarization-span-revision";
const GRAPH_UPDATE = "graph-update";
const GRAPH_DELTA = "graph-delta";

/** Stable identity for a cluster across refreshes (the ledger rebuilds
 * `SessionSpeakerCluster` values; the `ClusterRef` triple is its key). */
function clusterKey(cluster: ClusterRef): string {
  return `${cluster.provider}:${cluster.timeline_id}:${cluster.speaker_id}`;
}

/** Binding-state badge copy key for a cluster's `SpeakerBindingState`. */
const STATE_LABEL_KEYS = {
  anonymous: "speakerIdentity.state.anonymous",
  suggested: "speakerIdentity.state.suggested",
  bound: "speakerIdentity.state.bound",
  user_confirmed: "speakerIdentity.state.userConfirmed",
} as const;

function SpeakerIdentityPanel() {
  const { t } = useTranslation();
  const getSessionId = useAudioGraphStore((s) => s.getSessionId);
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);

  // `null` = not loaded yet (or no session); `[]` = loaded, no clusters.
  const [clusters, setClusters] = useState<SessionSpeakerCluster[] | null>(
    null,
  );
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** `role="alert"` — the latest mutation/list failure, verbatim from the
   * backend via `errorToMessage`. */
  const [mutationError, setMutationError] = useState<string | null>(null);
  /** `role="status"` — the latest successful manual edit. */
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  /** Cluster key of the in-flight mutation; `null` = idle. While set, every
   * action button is disabled (one atomic relabel batch at a time). */
  const [pendingKey, setPendingKey] = useState<string | null>(null);
  /** Rename inline-form draft, per cluster key. `undefined` = form closed. */
  const [renameDraft, setRenameDraft] = useState<{
    key: string;
    label: string;
  } | null>(null);
  /** "Same as…" target draft, per cluster key (selected option value). */
  const [mergeTarget, setMergeTarget] = useState<Record<string, string>>({});
  /** Monotonic refresh epoch — a stale `list` response (superseded by a
   * newer refresh or an unmount) must never overwrite newer state. */
  const refreshEpochRef = useRef(0);

  /** Re-read the session id + cluster list from the backend. Stale-response
   * safe: every await re-checks the epoch before committing state. */
  const refresh = useCallback(async () => {
    const epoch = ++refreshEpochRef.current;
    let id: string | null = null;
    try {
      id = await getSessionId();
    } catch {
      id = null;
    }
    if (epoch !== refreshEpochRef.current) return;
    id = typeof id === "string" && id.length > 0 ? id : null;
    setSessionId(id);
    if (id === null) {
      setClusters(null);
      setLoadError(null);
      return;
    }
    try {
      const list = await invoke<SessionSpeakerCluster[]>(
        "list_session_speaker_clusters",
        { sessionId: id },
      );
      if (epoch !== refreshEpochRef.current) return;
      setClusters(Array.isArray(list) ? list : []);
      setLoadError(null);
    } catch (error) {
      if (epoch !== refreshEpochRef.current) return;
      setClusters([]);
      setLoadError(errorToMessage(error));
    }
  }, [getSessionId]);

  // Initial load + capture start/stop (a stop/start rolls the session id).
  // biome-ignore lint/correctness/useExhaustiveDependencies: isCapturing is an intentional refresh trigger — a capture stop/start rolls the session id (same convention as LiveTranscript.tsx)
  useEffect(() => {
    void refresh();
  }, [refresh, isCapturing]);

  // Speaker/graph revisions rename clusters underneath us — re-read the
  // ledger on every backend emission so the list stays live.
  useEffect(() => {
    let cancelled = false;
    const offs: Array<() => void> = [];
    void (async () => {
      for (const name of [
        DIARIZATION_SPAN_REVISION,
        GRAPH_UPDATE,
        GRAPH_DELTA,
      ]) {
        const off = await listen(name, () => {
          void refresh();
        });
        if (cancelled) {
          off();
          return;
        }
        offs.push(off);
      }
    })();
    return () => {
      cancelled = true;
      for (const off of offs) off();
    };
  }, [refresh]);

  /** Run one manual identity edit: invoke, then re-read the ledger (the
   * backend is the source of truth — no local label patching), then surface
   * the outcome in the status/alert regions. */
  const runMutation = useCallback(
    async (key: string, action: () => Promise<unknown>, success: string) => {
      setMutationError(null);
      setStatusMessage(null);
      if (sessionId === null) {
        setMutationError(t("speakerIdentity.noSession"));
        return;
      }
      setPendingKey(key);
      try {
        await action();
        await refresh();
        setStatusMessage(success);
        setRenameDraft(null);
        setMergeTarget((prev) => {
          if (!(key in prev)) return prev;
          const next = { ...prev };
          delete next[key];
          return next;
        });
      } catch (error) {
        setMutationError(errorToMessage(error));
      } finally {
        setPendingKey(null);
      }
    },
    [refresh, sessionId, t],
  );

  const busy = pendingKey !== null;

  const submitRename = (cluster: SessionSpeakerCluster) => {
    const key = clusterKey(cluster.cluster);
    const label = renameDraft?.key === key ? renameDraft.label : "";
    const trimmed = label.trim();
    if (trimmed === "") {
      // Empty name is rejected client-side — the backend would accept it
      // as a label wipe, which is not what a rename form promises.
      setMutationError(t("speakerIdentity.renameEmpty"));
      return;
    }
    void runMutation(
      key,
      () =>
        invoke("relabel_speaker_cluster", {
          sessionId,
          cluster: cluster.cluster,
          label: trimmed,
        }),
      t("speakerIdentity.renamed", { label: trimmed }),
    );
  };

  return (
    <section
      className="panel flex-1 min-h-0 flex flex-col"
      aria-label={t("speakerIdentity.label")}
    >
      <div className="flex items-center justify-between mb-[10px]">
        <h3 className="panel-title">{t("speakerIdentity.title")}</h3>
        {clusters !== null && clusters.length > 0 && (
          <span className="text-xs font-semibold bg-(--tint-accent-info) text-(--text-on-tint-info) py-px px-(--space-4) rounded-xl min-w-[22px] text-center">
            {clusters.length}
          </span>
        )}
      </div>
      {mutationError !== null && (
        <p
          role="alert"
          className="text-xs m-0 mb-[6px] py-(--space-2) px-(--space-3) text-(--text-on-tint-danger) bg-(--tint-danger) border border-(--tint-border-danger) rounded-sm"
        >
          {mutationError}
        </p>
      )}
      {statusMessage !== null && (
        <p role="status" className="text-xs text-text-muted m-0 mb-[6px]">
          {statusMessage}
        </p>
      )}
      {loadError !== null && (
        <p role="alert" className="text-xs m-0 mb-[6px]">
          {loadError}
        </p>
      )}
      {sessionId === null ? (
        <p className="panel-empty">{t("speakerIdentity.noSession")}</p>
      ) : clusters === null || clusters.length === 0 ? (
        <p className="panel-empty">{t("speakerIdentity.empty")}</p>
      ) : (
        <ul className="list-none m-0 p-0 mt-0 flex flex-col gap-(--space-4) overflow-y-auto">
          {clusters.map((cluster) => {
            const key = clusterKey(cluster.cluster);
            const renaming = renameDraft?.key === key;
            const others = clusters.filter(
              (other) => clusterKey(other.cluster) !== key,
            );
            const canUnbind =
              cluster.state === "bound" || cluster.state === "user_confirmed";
            return (
              <li
                key={key}
                className="flex flex-col gap-(--space-2) py-(--space-3) px-(--space-4) rounded-sm"
                data-cluster-key={key}
              >
                <div className="flex items-center gap-(--space-3)">
                  <span className="text-md text-text-primary overflow-hidden text-ellipsis whitespace-nowrap">
                    {cluster.label}
                  </span>
                  <span className="text-xs font-semibold bg-bg-tertiary text-text-secondary py-px px-(--space-3) rounded-[3px] shrink-0">
                    {t(STATE_LABEL_KEYS[cluster.state])}
                  </span>
                  <span className="text-xs text-text-muted shrink-0 tabular-nums">
                    {t("speakerIdentity.spans", { count: cluster.span_count })}
                  </span>
                  {cluster.suggested_self && (
                    <span className="text-xs text-text-muted shrink-0">
                      {t("speakerIdentity.suggestedSelf")}
                    </span>
                  )}
                </div>
                <div className="flex flex-wrap items-center gap-(--space-3)">
                  {renaming ? (
                    // Inline rename form — Enter submits, Escape cancels.
                    // The input's accessible name carries the cluster label
                    // so a screen reader announces which speaker is edited.
                    <>
                      <input
                        value={renameDraft.label}
                        aria-label={t("speakerIdentity.renamePrompt", {
                          label: cluster.label,
                        })}
                        className="settings-input"
                        onChange={(e) =>
                          setRenameDraft({
                            key,
                            label: e.target.value,
                          })
                        }
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            submitRename(cluster);
                          } else if (e.key === "Escape") {
                            e.preventDefault();
                            setRenameDraft(null);
                            setMutationError(null);
                          }
                        }}
                      />
                      <Button
                        size="sm"
                        variant="primary"
                        disabled={busy}
                        onClick={() => submitRename(cluster)}
                      >
                        {t("speakerIdentity.renameSave")}
                      </Button>
                      <Button
                        size="sm"
                        disabled={busy}
                        onClick={() => setRenameDraft(null)}
                      >
                        {t("speakerIdentity.renameCancel")}
                      </Button>
                    </>
                  ) : (
                    <Button
                      size="sm"
                      disabled={busy}
                      onClick={() => {
                        setRenameDraft({ key, label: cluster.label });
                        setMutationError(null);
                      }}
                    >
                      {t("speakerIdentity.rename")}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    disabled={busy}
                    onClick={() =>
                      void runMutation(
                        key,
                        () =>
                          invoke("mark_cluster_is_self", {
                            sessionId,
                            cluster: cluster.cluster,
                          }),
                        t("speakerIdentity.markedSelf"),
                      )
                    }
                  >
                    {t("speakerIdentity.thisIsMe")}
                  </Button>
                  {others.length > 0 && (
                    <>
                      <select
                        value={mergeTarget[key] ?? ""}
                        aria-label={t("speakerIdentity.sameAsPrompt", {
                          label: cluster.label,
                        })}
                        className="settings-input"
                        disabled={busy}
                        onChange={(e) =>
                          setMergeTarget((prev) => ({
                            ...prev,
                            [key]: e.target.value,
                          }))
                        }
                      >
                        <option value="">
                          {t("speakerIdentity.sameAsPrompt", {
                            label: cluster.label,
                          })}
                        </option>
                        {others.map((other) => (
                          <option
                            key={clusterKey(other.cluster)}
                            value={clusterKey(other.cluster)}
                          >
                            {other.label}
                          </option>
                        ))}
                      </select>
                      <Button
                        size="sm"
                        disabled={busy || mergeTarget[key] === undefined}
                        onClick={() => {
                          const targetKey = mergeTarget[key];
                          const target = others.find(
                            (other) => clusterKey(other.cluster) === targetKey,
                          );
                          if (target === undefined) return;
                          void runMutation(
                            key,
                            () =>
                              invoke("merge_speaker_clusters", {
                                sessionId,
                                cluster: cluster.cluster,
                                into: target.cluster,
                              }),
                            t("speakerIdentity.merged"),
                          );
                        }}
                      >
                        {t("speakerIdentity.merge")}
                      </Button>
                    </>
                  )}
                  {canUnbind && (
                    <Button
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void runMutation(
                          key,
                          () =>
                            invoke("unbind_speaker_cluster", {
                              sessionId,
                              cluster: cluster.cluster,
                            }),
                          t("speakerIdentity.unbound"),
                        )
                      }
                    >
                      {t("speakerIdentity.unbind")}
                    </Button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

export default SpeakerIdentityPanel;
