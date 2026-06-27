import { fireEvent, render, screen } from "@testing-library/react";
import type { TFunction } from "i18next";
import { describe, expect, it, vi } from "vitest";
import SecretCredentialControl, {
  AwsCredentialControl,
} from "./SecretCredentialControl";

const t = ((key: string) => {
  const translations: Record<string, string> = {
    "settings.buttons.clearSavedAwsKeys": "Clear saved AWS keys",
    "settings.credentialConfirm.awsKeysLabel": "AWS credentials",
    "settings.credentialControl.add": "Add key",
    "settings.credentialControl.addAws": "Add AWS keys",
    "settings.credentialControl.awsMissingHint":
      "Add access keys before testing AWS providers.",
    "settings.credentialControl.awsSavedHint":
      "Saved AWS keys will be used for requests.",
    "settings.credentialControl.cancel": "Cancel",
    "settings.credentialControl.clear": "Clear saved key",
    "settings.credentialControl.missingHint": "Add a key before testing.",
    "settings.credentialControl.replace": "Replace key",
    "settings.credentialControl.replaceAws": "Replace AWS keys",
    "settings.credentialControl.savedHint":
      "Saved key will be used for requests.",
    "settings.credentialControl.status.missing": "Missing",
    "settings.credentialControl.status.saved": "Saved",
    "settings.fields.accessKeyId": "Access key ID",
    "settings.fields.secretAccessKey": "Secret access key",
    "settings.fields.sessionTokenOptional": "Session token (optional)",
    "settings.placeholders.sessionTokenHint": "Optional session token",
  };
  return translations[key] ?? key;
}) as TFunction;

describe("SecretCredentialControl", () => {
  it("adds provider context and status descriptions to saved single-key controls", () => {
    render(
      <SecretCredentialControl
        id="deepgram-api-key"
        label="Deepgram API key"
        value=""
        onChange={vi.fn()}
        saved
        t={t}
      />,
    );

    const replaceButton = screen.getByRole("button", {
      name: "Replace key: Deepgram API key",
    });
    expect(replaceButton).toHaveAttribute(
      "aria-describedby",
      "deepgram-api-key-credential-status deepgram-api-key-credential-hint",
    );
    expect(replaceButton).toHaveAccessibleDescription(
      /Saved Saved key will be used for requests\./i,
    );
    expect(screen.getByText("Saved")).toHaveAttribute(
      "id",
      "deepgram-api-key-credential-status",
    );
    expect(
      screen.getByText("Saved key will be used for requests."),
    ).toHaveAttribute("id", "deepgram-api-key-credential-hint");

    fireEvent.click(replaceButton);

    const input = screen.getByLabelText("Deepgram API key");
    expect(input).toHaveAttribute(
      "aria-describedby",
      "deepgram-api-key-credential-status deepgram-api-key-credential-hint",
    );
    expect(input).toHaveAccessibleDescription(
      /Saved Saved key will be used for requests\./i,
    );
  });

  it("describes AWS credential actions and password inputs with shared status context", () => {
    render(
      <AwsCredentialControl
        accessKeyId="aws-access-key"
        secretKeyId="aws-secret-key"
        sessionTokenId="aws-session-token"
        accessKey=""
        secretKey=""
        sessionToken=""
        onAccessKeyChange={vi.fn()}
        onSecretKeyChange={vi.fn()}
        onSessionTokenChange={vi.fn()}
        saved
        sessionTokenSaved={false}
        t={t}
        onClear={vi.fn()}
      />,
    );

    const replaceButton = screen.getByRole("button", {
      name: "Replace AWS keys: AWS credentials",
    });
    expect(replaceButton).toHaveAttribute(
      "aria-describedby",
      "aws-access-key-credential-status aws-access-key-credential-hint",
    );
    expect(replaceButton).toHaveAccessibleDescription(
      /Saved Saved AWS keys will be used for requests\./i,
    );

    const clearButton = screen.getByRole("button", {
      name: "Clear saved AWS keys: AWS credentials",
    });
    expect(clearButton).toHaveAttribute(
      "aria-describedby",
      "aws-access-key-credential-status aws-access-key-credential-hint",
    );

    fireEvent.click(replaceButton);

    for (const label of [
      "Access key ID",
      "Secret access key",
      "Session token (optional)",
    ]) {
      const input = screen.getByLabelText(label);
      expect(input).toHaveAttribute(
        "aria-describedby",
        "aws-access-key-credential-status aws-access-key-credential-hint",
      );
      expect(input).toHaveAccessibleDescription(
        /Saved Saved AWS keys will be used for requests\./i,
      );
    }
  });
});
