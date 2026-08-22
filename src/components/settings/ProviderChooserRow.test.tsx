import { render, screen, within } from "@testing-library/react";
import i18n from "i18next";
import { describe, expect, it } from "vitest";
import "../../i18n";
import { GENERATED_PROVIDER_REGISTRY } from "../../generated/providerRegistry";
import type { ProviderReadiness } from "../../types";
import ProviderChooserRow, {
  providerChooserAnnotationId,
} from "./ProviderChooserRow";

const t = i18n.getFixedT("en");

function requireProvider(id: string) {
  const descriptor = GENERATED_PROVIDER_REGISTRY.find(
    (provider) => provider.id === id,
  );
  if (!descriptor) throw new Error(`Expected ${id} in the registry`);
  return descriptor;
}

const DEEPGRAM = requireProvider("asr.deepgram");
const LOCAL_WHISPER = requireProvider("asr.local_whisper");
const CLOUD_API = requireProvider("asr.api");

function renderRow(props: Partial<Parameters<typeof ProviderChooserRow>[0]>) {
  const annotationId = providerChooserAnnotationId(
    (props.descriptor ?? LOCAL_WHISPER).id,
  );
  return render(
    <ProviderChooserRow
      descriptor={LOCAL_WHISPER}
      active={false}
      activeReadiness={null}
      credentialPresence={{}}
      t={t}
      {...props}
    >
      <label className="settings-radio">
        <input
          type="radio"
          name="chooser-row-test"
          checked={props.active ?? false}
          aria-describedby={annotationId}
          onChange={() => {}}
        />
        <span>{(props.descriptor ?? LOCAL_WHISPER).display_name}</span>
      </label>
    </ProviderChooserRow>,
  );
}

describe("ProviderChooserRow — annotated chooser (settings T3, audio-graph-9d2b)", () => {
  it("wires the radio to its annotation via aria-describedby, never inside the label", () => {
    renderRow({ descriptor: LOCAL_WHISPER });

    const radio = screen.getByRole("radio", { name: /^local whisper$/i });
    const describedById = radio.getAttribute("aria-describedby");
    expect(describedById).toBeTruthy();
    const annotation = document.getElementById(describedById as string);
    expect(annotation).not.toBeNull();
    // The annotation node is a SIBLING of the label, not a descendant of it —
    // moving it inside the label would change the radio's accessible name
    // (the tripwire the 7 anchored radio-name tests in SettingsPage.test.tsx
    // guard against).
    expect(annotation).not.toBe(radio.closest("label"));
    expect(radio.closest("label")?.contains(annotation)).toBe(false);
    // The accessible name is exactly the label text — no annotation leakage.
    expect(radio).toHaveAccessibleName("Local Whisper");
  });

  it("caps a non-active row at Axes 1-2 — data boundary + credential presence, never a readiness/Axis-3 chip", () => {
    renderRow({
      descriptor: DEEPGRAM,
      active: false,
      credentialPresence: { deepgram_api_key: { present: false } },
    });

    const annotation = document.getElementById(
      providerChooserAnnotationId(DEEPGRAM.id),
    ) as HTMLElement;
    const chips = annotation.querySelectorAll(".ag-chip");
    expect(chips).toHaveLength(2);
    // Axis 1 — data boundary, never `success`.
    expect(within(annotation).getByText(/vendor cloud/i)).toHaveAttribute(
      "data-tone",
      "accent",
    );
    // Axis 2 — credential PRESENCE (not readiness). Never `success` — a
    // missing key is a `warning`, not a fabricated "Ready".
    const credentialChip = within(annotation).getByText(/needs key/i);
    expect(credentialChip).toHaveAttribute("data-tone", "warning");
    expect(within(annotation).queryByText(/^ready$/i)).not.toBeInTheDocument();
  });

  it("swaps the credential chip for the T2-derived Axis-3 readiness chip only on the active row", () => {
    const readiness: ProviderReadiness = {
      provider_id: "asr.deepgram",
      status: "ready",
      message: "Deepgram key is valid",
      checked_at: Date.now(),
      stale: false,
      credential_epoch: 0,
      credentials: [{ key: "deepgram_api_key", present: true }],
    };

    renderRow({
      descriptor: DEEPGRAM,
      active: true,
      activeReadiness: readiness,
      credentialPresence: { deepgram_api_key: { present: true } },
    });

    const annotation = document.getElementById(
      providerChooserAnnotationId(DEEPGRAM.id),
    ) as HTMLElement;
    const readyChip = within(annotation).getByText(/^ready$/i);
    expect(readyChip).toHaveAttribute("data-tone", "success");
    // No separate credential-presence chip on the active row (cap of 2:
    // boundary + Axis 3, not boundary + Axis 2 + Axis 3).
    expect(
      within(annotation).queryByText(/key saved|needs key/i),
    ).not.toBeInTheDocument();
  });

  it("demotes a stale active-row readiness the same way ProviderReadinessPanel does — the two can never disagree", () => {
    const staleReadiness: ProviderReadiness = {
      provider_id: "asr.deepgram",
      status: "ready",
      message: "Deepgram key is valid",
      checked_at: Date.now(),
      stale: true,
      credential_epoch: 0,
      credentials: [{ key: "deepgram_api_key", present: true }],
    };

    renderRow({
      descriptor: DEEPGRAM,
      active: true,
      activeReadiness: staleReadiness,
      credentialPresence: { deepgram_api_key: { present: true } },
    });

    const annotation = document.getElementById(
      providerChooserAnnotationId(DEEPGRAM.id),
    ) as HTMLElement;
    expect(within(annotation).queryByText(/^ready$/i)).not.toBeInTheDocument();
    expect(within(annotation).getByText(/unchecked/i)).toHaveAttribute(
      "data-tone",
      "neutral",
    );
  });

  it("renders the local-model requirement line for a local_files provider", () => {
    renderRow({ descriptor: LOCAL_WHISPER });
    const annotation = document.getElementById(
      providerChooserAnnotationId(LOCAL_WHISPER.id),
    ) as HTMLElement;
    expect(
      within(annotation).getByText(/needs a local model file selected/i),
    ).toBeInTheDocument();
  });

  it("renders the default-model requirement line for a remote provider with a fixed default", () => {
    renderRow({ descriptor: DEEPGRAM });
    const annotation = document.getElementById(
      providerChooserAnnotationId(DEEPGRAM.id),
    ) as HTMLElement;
    expect(
      within(annotation).getByText(/default model: nova-3/i),
    ).toBeInTheDocument();
  });

  it("renders no requirement line when the provider has neither a local-files catalog nor a default model", () => {
    renderRow({ descriptor: CLOUD_API });
    const annotation = document.getElementById(
      providerChooserAnnotationId(CLOUD_API.id),
    ) as HTMLElement;
    expect(
      annotation.querySelector(".settings-radio-annotation__line"),
    ).not.toBeInTheDocument();
  });
});
