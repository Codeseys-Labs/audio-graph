import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { ARTIFACT_REFUSAL_CLASSES } from "../utils/errorToMessage";
import { ArtifactRefusalNotice } from "./ArtifactRefusalNotice";

describe("ArtifactRefusalNotice", () => {
  afterEach(async () => {
    await i18n.changeLanguage("en");
  });

  it.each([
    "en",
    "pt",
  ])("labels every refusal class in %s, never showing a raw class name", async (language) => {
    await i18n.changeLanguage(language);
    for (const artifactClass of ARTIFACT_REFUSAL_CLASSES) {
      const { unmount } = render(
        <ArtifactRefusalNotice
          status={{
            type: "refused",
            artifactClass,
            sizeBytes: 5 * 1024 * 1024,
            ceilingBytes: 1024 * 1024,
          }}
        />,
      );
      const body = screen.getByTestId("artifact-refusal-notice").textContent;
      // A snake_case identifier in the copy means the label key is missing.
      expect(body).not.toContain(artifactClass);
      expect(body).not.toMatch(/[a-z]+_[a-z]+/);
      unmount();
    }
  });

  it("falls back to a generic label, not the raw class, for a class outside the closed set", () => {
    render(
      <ArtifactRefusalNotice
        status={{
          type: "refused",
          artifactClass: "../../something_else",
          sizeBytes: 2,
          ceilingBytes: 1,
        }}
      />,
    );
    const body = screen.getByTestId("artifact-refusal-notice").textContent;
    expect(body).not.toContain("something_else");
    expect(body).toContain("data");
  });
});
