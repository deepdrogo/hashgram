// Self-update over GitHub Releases. One place owns the state so the Settings
// tab, the start-up check and the shell banner all agree on what is going on.
//
// The manifest (latest.json) and the installer come from
// https://github.com/deepdrogo/hashgram_windows/releases; both are refused
// unless the minisign signature verifies against the public key compiled into
// this build (tauri.conf.json → plugins.updater.pubkey). No other endpoint
// is ever contacted.
import { createRoot, createSignal } from "solid-js";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ipc } from "./ipc";
import { store } from "./store";

export type UpdatePhase = "idle" | "checking" | "up-to-date" | "available" | "downloading" | "installing" | "error";

export interface UpdateProgress {
  done: number;
  total: number | null;
}

/** Delay before the automatic check on start, so it never competes with the cold start. */
export const START_CHECK_DELAY_MS = 6_000;

/** Compares two dotted versions; > 0 when `a` is newer than `b`. */
export function compareVersions(a: string, b: string): number {
  const pa = a.replace(/^v/, "").split(/[.-]/).map((x) => Number.parseInt(x, 10) || 0);
  const pb = b.replace(/^v/, "").split(/[.-]/).map((x) => Number.parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

function createUpdates() {
  const [phase, setPhase] = createSignal<UpdatePhase>("idle");
  const [available, setAvailable] = createSignal<Update | null>(null);
  const [progress, setProgress] = createSignal<UpdateProgress>({ done: 0, total: null });
  const [error, setError] = createSignal("");
  const [checkedAt, setCheckedAt] = createSignal<number | null>(null);
  const [dismissed, setDismissed] = createSignal(false);
  let startChecked = false;

  const busy = () => phase() === "checking" || phase() === "downloading" || phase() === "installing";

  /** Asks the endpoint for a signed manifest. Returns the update when one is newer than this build. */
  const checkNow = async (): Promise<Update | null> => {
    if (busy()) return available();
    if (!store.status()?.updater_configured) {
      setError("This build has no updater public key; refusing unsigned manifests.");
      setPhase("error");
      return null;
    }
    setPhase("checking");
    setError("");
    try {
      const u = await check({ timeout: 20_000 });
      setCheckedAt(Date.now());
      if (!u) {
        setAvailable(null);
        setPhase("up-to-date");
        return null;
      }
      setAvailable(u);
      setDismissed(false);
      setPhase("available");
      return u;
    } catch (e) {
      setError(String(e));
      setPhase("error");
      return null;
    }
  };

  /** Downloads, verifies the signature, runs the installer passively and restarts. */
  const install = async () => {
    const u = available();
    if (!u || busy()) return;
    if (await ipc.txHasPending().catch(() => false)) {
      setError("A transaction is pending; the update waits until it is committed.");
      return;
    }
    setError("");
    setPhase("downloading");
    setProgress({ done: 0, total: null });
    try {
      await u.downloadAndInstall((ev) => {
        if (ev.event === "Started") setProgress({ done: 0, total: ev.data.contentLength ?? null });
        else if (ev.event === "Progress") setProgress((p) => ({ done: p.done + ev.data.chunkLength, total: p.total }));
        else if (ev.event === "Finished") setPhase("installing");
      });
      // On Windows the installer closes the app itself; this only runs elsewhere.
      await relaunch();
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  /** Runs once per process when the user left "check on start" enabled. */
  const checkOnStart = async () => {
    if (startChecked) return;
    startChecked = true;
    const s = store.settings();
    if (!s?.updates.auto_check) return;
    if (!store.status()?.updater_configured) return;
    await new Promise((r) => setTimeout(r, START_CHECK_DELAY_MS));
    const u = await checkNow();
    if (u) store.toast(`Hashgram ${u.version} is available — Settings → Updates`);
  };

  return {
    phase,
    available,
    progress,
    error,
    checkedAt,
    dismissed,
    setDismissed,
    busy,
    checkNow,
    install,
    checkOnStart,
  };
}

export const updates = createRoot(createUpdates);
