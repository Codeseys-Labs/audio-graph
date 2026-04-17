import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
// Initialize i18next before React mounts so the first render has
// translations available. Side-effect import — do not remove.
import "./i18n";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
