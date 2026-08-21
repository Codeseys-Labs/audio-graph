/**
 * System drawer (SHELL-R3, plan §R3, ADR-0046) — opened from NowStrip's
 * composite health chip. Hosts `ProjectionRuntimeStatusPanel` +
 * `TokenUsagePanel` + the full per-stage pipeline detail
 * (`PipelineStageDetail`, shared with `PipelineStatusBar`'s footer fold),
 * so the detail `PipelineStatusBar` collapses away during healthy capture
 * is always one click away here — the 50e3 fold's "no regression to
 * diagnostics" half.
 *
 * Hand-rolled `useFocusTrap` + Escape, per the ratified D5 decision — NO
 * Radix dialog. `PopoverOverlay` (the component this pattern used to live
 * in) is RETIRED and deleted outright, which is what fixes the ≤1120px
 * anchor-overlap bug: rather than re-anchoring a top-right pop-down to a
 * moving trigger, this is a full-height, non-anchored side panel. Its two
 * former consumers' disposition:
 *   - `TokenUsagePanel` moves here.
 *   - `AgentProposalsPanel`'s pop-down is NOT re-homed here — it already
 *     has an inline surface (`App.tsx`'s `workspace-panel__assist` section,
 *     shown whenever `hasAgentActivity` during Capture) that the pop-down
 *     duplicated. `NowStrip`'s Agent toggle button is removed outright, not
 *     ported; the pending-proposals count badge it carried has no
 *     replacement in this unit (a real, small reach reduction, not an
 *     oversight — see the SHELL-R3 landing notes).
 *
 * Parent: `App.tsx`, conditionally on `systemDrawerOpen`.
 */
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useFocusTrap } from "../hooks/useFocusTrap";
import IconButton from "./IconButton";
import { PipelineStageDetail } from "./PipelineStatusBar";
import ProjectionRuntimeStatusPanel from "./ProjectionRuntimeStatusPanel";
import TokenUsagePanel from "./TokenUsagePanel";

interface SystemDrawerProps {
  onClose: () => void;
}

export default function SystemDrawer({ onClose }: SystemDrawerProps) {
  const { t } = useTranslation();
  const ref = useFocusTrap<HTMLDivElement>();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <>
      <div
        className="fixed inset-0 z-[var(--z-modal)] bg-(--scrim-color)"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={t("systemDrawer.title")}
        tabIndex={-1}
        className="fixed top-0 right-0 h-full w-[380px] max-w-[calc(100vw-24px)] z-[var(--z-modal)] bg-bg-secondary border-l border-(--edge) shadow-(--shadow-overlay) flex flex-col overflow-hidden"
      >
        <div className="ag-panel-head">
          <h2 className="ag-panel-head__title">{t("systemDrawer.title")}</h2>
          <IconButton
            icon="close"
            label={t("systemDrawer.close")}
            variant="ghost"
            onClick={onClose}
          />
        </div>
        <div className="flex-1 overflow-auto p-(--space-5) flex flex-col gap-(--space-6)">
          <section aria-label={t("systemDrawer.pipelineHeading")}>
            <h3 className="ag-label mb-(--space-3)">
              {t("systemDrawer.pipelineHeading")}
            </h3>
            <div className="flex flex-wrap items-center gap-(--space-1)">
              <PipelineStageDetail />
            </div>
          </section>
          <ProjectionRuntimeStatusPanel />
          <TokenUsagePanel />
        </div>
      </div>
    </>
  );
}
