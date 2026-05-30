import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
// Initialize i18next before React mounts so the first render has
// translations available. Side-effect import — do not remove.
import "./i18n";
import { applyTheme, readStoredTheme } from "./theme";

// Apply the persisted theme before the first paint so there is no
// dark→light flash when a light-theme user reloads (ADR-0009, Wave 4).
applyTheme(readStoredTheme());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
