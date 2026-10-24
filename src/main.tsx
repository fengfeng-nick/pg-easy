import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "@/styles/tokens.css";
import "@/styles/base.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "@/App";

// The window background is painted from CSS; disabling the browser's own
// context menu keeps the app feeling native. Text inputs keep their menu.
document.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement;
  if (target.closest("input, textarea, [contenteditable='true'], .cm-content")) return;
  event.preventDefault();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
