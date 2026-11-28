import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import type { StageStatus } from "../../types";
import DiarizationDegradationNotice from "./DiarizationDegradationNotice";

it("renders the closed diagnostic for an unrecognized diarization reason", () => {
  const reason = "private reason canary";
  render(
    <DiarizationDegradationNotice
      status={
        {
          type: "Degraded",
          reason,
          diagnostic: {
            kind: "runtime",
            detail: {
              code: "unsupported",
              retryable: false,
              recovery_action: "none",
              context: { operation: "transcription" },
            },
          },
        } as unknown as Extract<StageStatus, { type: "Degraded" }>
      }
    />,
  );
  expect(
    screen.getByText("The provider does not support this operation."),
  ).toBeInTheDocument();
  expect(document.body.innerHTML).not.toContain(reason);
});
