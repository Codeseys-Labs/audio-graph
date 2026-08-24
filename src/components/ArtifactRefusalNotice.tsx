/**
 * The Notes/Graph lens byte-ceiling refusal notice (seed audio-graph-4fa5
 * deliverables a/b).
 *
 * Renders in place of `NotesPanel` / `KnowledgeGraphViewer` when the
 * corresponding lens's own artifact fetch (`loadSessionNotesArtifacts` /
 * `loadSessionGraphArtifact`, `store/index.ts`) resolves to a
 * `SessionLensArtifactStatus` of `type: "refused"` — the backend's
 * per-artifact byte ceiling rejected the artifact because it predates the
 * artifact-size fix (seed audio-graph-cfa1: unbounded per-fact basis
 * growth). Never a blank panel, never a crash — the tone-law-adjacent
 * pattern this mirrors is `DiarizationDegradationNotice`: a stable
 * snake_case class name from the backend, translated here, never raw prose
 * composed in Rust.
 *
 * Parent: `SessionsBrowser`'s Notes/Graph lens panels, in place of the
 * normal panel component for that lens.
 */
import { useTranslation } from "react-i18next";
import type { SessionLensArtifactStatus } from "../types";
import Icon from "./Icon";

export interface ArtifactRefusalNoticeProps {
  /** Narrowed to the `refused` arm by the caller before rendering this
   * component (`status.type === "refused"`). */
  status: Extract<SessionLensArtifactStatus, { type: "refused" }>;
}

function formatMb(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(1);
}

export function ArtifactRefusalNotice({ status }: ArtifactRefusalNoticeProps) {
  const { t } = useTranslation();

  const artifactLabel = t(
    `sessions.artifactRefusal.classLabel.${status.artifactClass}`,
    { defaultValue: status.artifactClass },
  );

  return (
    <section
      className="flex h-full flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center"
      role="status"
      data-testid="artifact-refusal-notice"
    >
      <span className="shrink-0" aria-hidden="true">
        <Icon name="warning" size={22} className="text-text-muted opacity-60" />
      </span>
      <p className="m-0 max-w-[420px] text-sm font-semibold text-text-primary">
        {t("sessions.artifactRefusal.title")}
      </p>
      <p className="m-0 max-w-[420px] text-sm text-text-secondary">
        {t("sessions.artifactRefusal.body", {
          artifactLabel,
          sizeMb: formatMb(status.sizeBytes),
          ceilingMb: formatMb(status.ceilingBytes),
        })}
      </p>
    </section>
  );
}

export default ArtifactRefusalNotice;
