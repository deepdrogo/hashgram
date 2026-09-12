// Global reactive state: tiny, explicit, no framework beyond Solid signals.
import { createSignal, createRoot } from "solid-js";
import { ipc, on, type AppStatus, type NetSnapshot, type Settings, type ChainHealth } from "./ipc";

export interface Toast {
  id: number;
  text: string;
  kind?: "info" | "error";
}

function createStore() {
  const [status, setStatus] = createSignal<AppStatus | null>(null);
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [net, setNet] = createSignal<NetSnapshot | null>(null);
  const [health, setHealth] = createSignal<ChainHealth | null>(null);
  const [toasts, setToasts] = createSignal<Toast[]>([]);
  const [locked, setLocked] = createSignal(true);
  const [pendingTx, setPendingTx] = createSignal<number>(0);
  let toastId = 0;

  const toast = (text: string, kind: Toast["kind"] = "info") => {
    const id = ++toastId;
    setToasts((t) => [...t, { id, text, kind }].slice(-4));
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 7000 : 3500);
  };

  const refreshStatus = async () => {
    try {
      const s = await ipc.appStatus();
      setStatus(s);
      setLocked(!s.unlocked);
      return s;
    } catch (e) {
      toast(String(e), "error");
      return null;
    }
  };
  const refreshSettings = async () => {
    try {
      setSettings(await ipc.settingsGet());
    } catch (e) {
      toast(String(e), "error");
    }
  };
  const refreshNet = async () => {
    try {
      setNet(await ipc.netSnapshot());
    } catch {
      /* the swarm may not be up yet */
    }
  };
  const refreshHealth = async (probe = false) => {
    try {
      setHealth(await ipc.chainHealth(probe));
    } catch {
      /* ignore */
    }
  };
  const refreshPending = async () => {
    try {
      const rows = await ipc.txRecent();
      setPendingTx(rows.filter((r) => r.state === "pending").length);
    } catch {
      /* ignore */
    }
  };

  let wired = false;
  const wire = async () => {
    if (wired) return;
    wired = true;
    await on("net:changed", () => {
      void refreshNet();
      void refreshHealth(false);
    });
    await on("session:locked", () => {
      setLocked(true);
      void refreshStatus();
      toast("Locked");
    });
    await on("settings:changed", () => {
      void refreshSettings();
      void refreshHealth(false);
    });
    await on("tx:update", (p) => {
      void refreshPending();
      if (p.state === "committed") toast(`Transaction committed at height ${p.height ?? "?"}`);
      if (p.state === "failed") toast(`Transaction failed: ${p.raw_log ?? ""}`, "error");
    });
  };

  return {
    status,
    settings,
    net,
    health,
    toasts,
    locked,
    pendingTx,
    setLocked,
    toast,
    refreshStatus,
    refreshSettings,
    refreshNet,
    refreshHealth,
    refreshPending,
    wire,
  };
}

export const store = createRoot(createStore);
