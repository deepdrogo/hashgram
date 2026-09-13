// File dialogs through the dialog plugin (the webview has no filesystem
// access; it only learns paths the user chose).
import { open, save, ask } from "@tauri-apps/plugin-dialog";

export async function pickFile(opts?: { multiple?: boolean; title?: string; filters?: { name: string; extensions: string[] }[] }): Promise<string[]> {
  const r = await open({ multiple: opts?.multiple ?? false, directory: false, title: opts?.title, filters: opts?.filters });
  if (!r) return [];
  return Array.isArray(r) ? r : [r];
}

export async function pickSavePath(defaultName: string, opts?: { title?: string; filters?: { name: string; extensions: string[] }[] }): Promise<string | null> {
  const r = await save({ defaultPath: defaultName, title: opts?.title, filters: opts?.filters });
  return r ?? null;
}

export async function confirm(message: string, title = "Hashgram"): Promise<boolean> {
  try {
    return await ask(message, { title, kind: "warning" });
  } catch {
    return window.confirm(message);
  }
}
