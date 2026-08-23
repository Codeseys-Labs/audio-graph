/**
 * The 586b degradation banner — the ONE consumer of the `notice` grid row
 * `layout.css`'s ticket W4 reserved with a `0px` height and zero visual
 * footprint (`.workspace-panel--capture`'s `grid-template-rows`; see that
 * file's comment on the wide-tier rule for the full history).
 *
 * Renders ONLY when the backend's `PipelineStatus.diarization` stage is the
 * `Degraded` variant (audio-graph-586b: `DiarizationSettings.mode` must never
 * be silently overridden by an unannounced Simple-backend fallback with no
 * notice).
 *
 * `status.reason` is a stable `snake_case` degradation-class code (e.g.
 * `"asset_not_downloaded"` — `StageStatus::Degraded.reason`,
 * `speech::DiarizationDegradationReason::as_wire_code`), NEVER English prose
 * composed in Rust (review follow-up, audio-graph-586b: the original fix
 * shipped hardcoded English here, bypassing the app's existing typed +
 * translated degradation vocabulary — see `SttFidelityDegradation` /
 * `ProviderReadinessPanel.tsx`'s identical `t(...)`-by-code pattern). This
 * component looks the code up against `pipeline.diarizationDegradedReason.<code>`
 * — a real, fully translated string in every locale naming the degradation
 * class and its remedy — never transcript content, and never a raw code or
 * untranslated fragment shown to the user.
 *
 * `status` is passed in (not read from the store here) so the ONE store read
 * this ticket needs lives in `ShellRailContentAside` alongside the sibling
 * `data-diarization-degraded` attribute it also drives on the grid
 * container — mirrors the existing `graphStripMode`/`liveDocumentVm` "lift
 * once, pass to both consumers" convention in that file (ticket W7/W5).
 *
 * Parent: `App.tsx`'s `ShellRailContentAside`, as the first child of
 * `.workspace-panel--capture` (before the four `WorkspaceTile`s), so its
 * `grid-area: notice` placement (via the `.workspace-notice` class,
 * `layout.css`) is independent of DOM/tab order.
 */
import { useTranslation } from "react-i18next";
import type { StageStatus } from "../../types";
import Icon from "../Icon";

export interface DiarizationDegradationNoticeProps {
  /** The full stage status — narrowed to the `Degraded` arm by the caller
   * before rendering this component (`status.type === "Degraded"`). Typed as
   * the full `StageStatus` union anyway so a caller passing the wrong stage
   * can't silently compile; `reason` below only exists on the `Degraded`
   * arm. */
  status: Extract<StageStatus, { type: "Degraded" }>;
}

export function DiarizationDegradationNotice({
  status,
}: DiarizationDegradationNoticeProps) {
  const { t } = useTranslation();

  return (
    <div
      className="workspace-notice"
      role="status"
      data-testid="diarization-degradation-notice"
    >
      <span className="shrink-0" aria-hidden="true">
        <Icon name="warning" />
      </span>
      <span className="ag-chip" data-tone="warning">
        {t("workspace.notice.diarizationDegradedTitle")}
      </span>
      <span className="workspace-notice__reason">
        {t(`pipeline.diarizationDegradedReason.${status.reason}`)}
      </span>
    </div>
  );
}

export default DiarizationDegradationNotice;
