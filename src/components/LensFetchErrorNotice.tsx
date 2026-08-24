/**
 * The Notes/Graph lens's generic (non-ceiling) fetch-error notice (fix-round
 * finding for seed audio-graph-4fa5): renders when
 * `loadSessionNotesArtifacts` / `loadSessionGraphArtifact` (`store/index.ts`)
 * resolve to a `SessionLensArtifactStatus` of `type: "error"` — any failure
 * that ISN'T the byte-ceiling refusal `ArtifactRefusalNotice` already covers.
 * The most important case this catches: `replay_projection_state_or_invalid`
 * (`commands.rs`) rejecting one or more canonical projection patches — a
 * real data-integrity signal that, before the lens split, blocked the WHOLE
 * session open with a visible error. Without this notice that failure
 * rendered as an ordinary empty Notes/Graph panel, indistinguishable from a
 * session with no notes.
 *
 * Parent: `SessionsBrowser`'s Notes/Graph lens panels, alongside
 * `ArtifactRefusalNotice` — one or the other renders in place of the normal
 * panel component depending on which status arm the lens's fetch resolved
 * to.
 */
import { useTranslation } from "react-i18next";
import type { SessionLensArtifactStatus } from "../types";
import Icon from "./Icon";

export interface LensFetchErrorNoticeProps {
  /** Narrowed to the `error` arm by the caller before rendering this
   * component (`status.type === "error"`). */
  status: Extract<SessionLensArtifactStatus, { type: "error" }>;
}

export function LensFetchErrorNotice({ status }: LensFetchErrorNoticeProps) {
  const { t } = useTranslation();

  return (
    <section
      className="flex h-full flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center"
      role="alert"
      data-testid="lens-fetch-error-notice"
    >
      <span className="shrink-0" aria-hidden="true">
        <Icon name="warning" size={22} className="text-text-muted opacity-60" />
      </span>
      <p className="m-0 max-w-[420px] text-sm font-semibold text-text-primary">
        {t("sessions.lensFetchError.title")}
      </p>
      <p className="m-0 max-w-[420px] text-sm text-text-secondary">
        {t("sessions.lensFetchError.body", { message: status.message })}
      </p>
    </section>
  );
}

export default LensFetchErrorNotice;
