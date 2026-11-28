/**
 * Localized copy for a closed runtime diagnostic: the code's message, its
 * recovery step when the diagnostic names one, and an optional caller hint
 * for the `rate_limited` code. The value is normalized first, so a
 * malformed payload renders the fixed internal copy; no raw wire value or
 * `runtime/<code>` token is ever rendered.
 */
import { useTranslation } from "react-i18next";
import { runtimeDiagnosticPresentation } from "../utils/runtimeDiagnostic";

export default function RuntimeDiagnosticText({
  diagnostic,
  rateLimitedHintKey,
}: {
  diagnostic: unknown;
  /** Translation key appended when the code is `rate_limited`. */
  rateLimitedHintKey?: string;
}) {
  const { t } = useTranslation();
  const presentation = runtimeDiagnosticPresentation(diagnostic);
  const lines = [t(presentation.translationKey)];
  if (presentation.recoveryTranslationKey) {
    lines.push(t(presentation.recoveryTranslationKey));
  }
  if (
    rateLimitedHintKey &&
    presentation.diagnostic.kind === "runtime" &&
    presentation.diagnostic.detail.code === "rate_limited"
  ) {
    lines.push(t(rateLimitedHintKey));
  }
  return <>{lines.join(" ")}</>;
}
