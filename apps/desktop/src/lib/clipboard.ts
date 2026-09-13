import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "./store";
import { t } from "./i18n";

let clearTimer: ReturnType<typeof setTimeout> | null = null;

/** Copies an address, hash or link. Never used for the 24 words. */
export async function copyText(text: string, label?: string) {
  try {
    await writeText(text);
    store.toast(label ?? t("copied"));
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      store.toast(label ?? t("copied"));
    } catch (e) {
      store.toast(`Copy failed: ${String(e)}`, "error");
    }
  }
}

/** Copies something sensitive (never the mnemonic) and clears it after the
 *  configured delay. */
export async function copySensitive(text: string) {
  const secs = store.settings()?.security.clipboard_clear_secs ?? 30;
  await copyText(text, `${t("copied")} — clears in ${secs} s`);
  if (clearTimer) clearTimeout(clearTimer);
  clearTimer = setTimeout(() => {
    void writeText("").catch(() => undefined);
  }, secs * 1000);
}
