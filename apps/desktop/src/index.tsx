/* @refresh reload */
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import "./styles/app.css";
import { App } from "./App";

// Uncaught errors go to the Rust log (and the dev console). Messages are
// ours; no secret ever passes through here.
const report = (message: string) => {
  void invoke("ui_log", { level: "error", message }).catch(() => undefined);
};
window.addEventListener("error", (e) => report(`ui error: ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => report(`ui unhandled rejection: ${String(e.reason)}`));

const root = document.getElementById("root");
if (root) {
  // No context menu in the chrome (content areas opt in), no zoom.
  window.addEventListener("contextmenu", (e) => {
    if (!(e.target as HTMLElement | null)?.closest("input, textarea, .selectable")) e.preventDefault();
  });
  render(() => <App />, root);
}
