import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "./store";

let clearTimer: ReturnType<typeof setTimeout> | null = null;

/** Copies an address, hash or link. Never used for the 24 words. */
export async function copyText(text: string, label = "Copied") {
  try {
    await writeText(text);
    store.toast(label);
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      store.toast(label);
    } catch (e) {
      store.toast(`Copy failed: ${String(e)}`, "error");
    }
  }
}

/** Copies something sensitive (never the mnemonic) and clears it after the
 *  configured delay. */
export async function copySensitive(text: string, label = "Copied · clears in 30 s") {
  const secs = store.settings()?.security.clipboard_clear_secs ?? 30;
  await copyText(text, label.replace("30", String(secs)));
  if (clearTimer) clearTimeout(clearTimer);
  clearTimer = setTimeout(() => {
    void writeText("").catch(() => undefined);
  }, secs * 1000);
}
