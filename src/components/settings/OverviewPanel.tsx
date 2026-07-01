/**
 * Overview rail section — the orientation "homepage" (blueprint §1.1, Phase 4).
 *
 * STEP 2 relocated the registry capability cards out of Overview into each
 * provider panel's advanced disclosure (blueprint §1.2). WS3 (ADR-0006 B1) then
 * moved the cross-provider readiness rollup into the Credentials panel, so this
 * panel now holds only the product-mode summary cards (the interactive "Modes"
 * selector). Reads everything from the settings controller via `useSettings()`.
 */

import ProductModeSummaryCards from "./ProductModeSummaryCards";

export default function OverviewPanel() {
  return <ProductModeSummaryCards />;
}
