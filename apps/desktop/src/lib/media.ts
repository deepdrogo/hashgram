// Media helpers: cached, hash-verified files served to the webview through
// Tauri's asset protocol. Only what Rust downloaded and verified is shown.
import { convertFileSrc } from "@tauri-apps/api/core";
import { ipc, type AttachmentView, type MediaView } from "./ipc";

const inflight = new Map<string, Promise<string>>();

export function assetUrl(path: string): string {
  return convertFileSrc(path);
}

/** Resolves a chat attachment to a displayable URL (downloads once). */
export function attachmentUrl(a: AttachmentView): Promise<string> {
  const k = `a:${a.cid}`;
  let p = inflight.get(k);
  if (!p) {
    p = ipc.chatAttachment(a).then(assetUrl);
    inflight.set(k, p);
    p.catch(() => inflight.delete(k));
  }
  return p;
}

/** Resolves public media to a displayable URL (downloads once). */
export function mediaUrl(m: MediaView): Promise<string> {
  const k = `m:${m.cid}`;
  let p = inflight.get(k);
  if (!p) {
    p = ipc.mediaFetch(m).then(assetUrl);
    inflight.set(k, p);
    p.catch(() => inflight.delete(k));
  }
  return p;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 ** 2).toFixed(1)} MB`;
}

export function durationLabel(ms: number): string {
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
