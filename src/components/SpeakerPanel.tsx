/**
 * Speakers panel — compact list of detected speakers with per-speaker
 * colour swatch, human-readable talk time, and segment count badge.
 *
 * The backend diarization worker (see `src-tauri/src/diarization/mod.rs`)
 * emits `SPEAKER_DETECTED` events which `useTauriEvents` funnels into the
 * store. This component simply reflects that state — it is purely
 * presentational and carries no local UI state.
 *
 * Store bindings: `speakers`.
 *
 * Parent: `App.tsx` left panel. No props.
 */
import { useAudioGraphStore } from "../store";
import { formatDuration } from "../utils/format";

function SpeakerPanel() {
  const speakers = useAudioGraphStore((s) => s.speakers);

  return (
    <section className="panel flex-1 min-h-0" aria-label="Detected speakers">
      <div className="flex items-center justify-between mb-[10px]">
        <h3 className="panel-title">Speakers</h3>
        {speakers.length > 0 && (
          <span className="text-xs font-semibold bg-(--tint-accent-info) text-(--text-on-tint-info) py-px px-(--space-4) rounded-[10px] min-w-[22px] text-center">
            {speakers.length}
          </span>
        )}
      </div>
      {speakers.length === 0 ? (
        <p className="panel-empty">No speakers detected yet</p>
      ) : (
        <ul className="list-none m-0 p-0 flex flex-col gap-(--space-2)">
          {speakers.map((speaker) => (
            <li
              key={speaker.id}
              className="flex items-center gap-(--space-4) py-(--space-3) px-(--space-4) rounded-sm transition-[background-color] duration-[120ms] ease-[ease] hover:bg-(--hover-overlay)"
            >
              <span
                className="w-[10px] h-[10px] rounded-full shrink-0"
                style={{ backgroundColor: speaker.color }}
                aria-hidden="true"
              />
              <div className="flex flex-col flex-1 min-w-0">
                <span className="text-md text-text-primary overflow-hidden text-ellipsis whitespace-nowrap">
                  {speaker.label}
                </span>
                <span className="text-xs text-text-muted">
                  {formatDuration(speaker.total_speaking_time)} ·{" "}
                  {speaker.segment_count} segments
                </span>
              </div>
              <span className="text-2xs font-semibold bg-bg-tertiary text-text-secondary py-px px-(--space-3) rounded-[3px] shrink-0">
                {speaker.segment_count}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export default SpeakerPanel;
