// Global reactive state: explicit signals, no framework beyond Solid.
import { createSignal, createRoot } from "solid-js";
import { ipc, on, errText, type AppStatus, type Settings, type SyncStatus, type SyncPhase, type FolderCounts, type SyncEvent, type IdentityStatus } from "./ipc";
import { setLocale } from "./i18n";

export interface Toast {
  id: number;
  text: string;
  kind?: "info" | "error";
  action?: { label: string; run: () => void };
}

export type Refresh = "mail" | "drive" | "people" | "feed" | "circles" | "spaces" | "wallet" | "network";

function createStore() {
  const [status, setStatus] = createSignal<AppStatus | null>(null);
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [sync, setSync] = createSignal<SyncStatus | null>(null);
  const [phase, setPhase] = createSignal<SyncPhase>("Offline");
  const [counts, setCounts] = createSignal<Record<string, FolderCounts>>({});
  const [requestsIn, setRequestsIn] = createSignal(0);
  const [toasts, setToasts] = createSignal<Toast[]>([]);
  const [locked, setLocked] = createSignal(true);
  const [pendingTx, setPendingTx] = createSignal<number>(0);
  const [balance, setBalance] = createSignal<string | null>(null);
  const [identity, setIdentity] = createSignal<IdentityStatus | null>(null);
  // A monotonically increasing tick per area; screens re-fetch when it changes.
  const [ticks, setTicks] = createSignal<Record<Refresh, number>>({ mail: 0, drive: 0, people: 0, feed: 0, circles: 0, spaces: 0, wallet: 0, network: 0 });
  let toastId = 0;
  let registrationReadyNotified = false;

  const toast = (text: string, kind: Toast["kind"] = "info", action?: Toast["action"]) => {
    const id = ++toastId;
    setToasts((t) => [...t, { id, text, kind, action }].slice(-4));
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 8000 : 3500);
  };
  const dismissToast = (id: number) => setToasts((t) => t.filter((x) => x.id !== id));

  const bump = (...areas: Refresh[]) =>
    setTicks((t) => {
      const n = { ...t };
      for (const a of areas) n[a] = (n[a] ?? 0) + 1;
      return n;
    });

  const applyAppearance = (s: Settings | null) => {
    const html = document.documentElement;
    const theme = s?.appearance.theme ?? "dark";
    const resolved = theme === "system" ? (window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark") : theme;
    html.dataset.theme = resolved;
    html.dataset.density = s?.appearance.density ?? "comfortable";
    html.dataset.reducedMotion = s?.appearance.reduced_motion ? "true" : "false";
    setLocale(s?.appearance.language === "ka" ? "ka" : "en");
  };

  const refreshStatus = async () => {
    try {
      const s = await ipc.appStatus();
      setStatus(s);
      setLocked(!s.unlocked);
      return s;
    } catch (e) {
      toast(errText(e), "error");
      return null;
    }
  };
  const refreshSettings = async () => {
    try {
      const s = await ipc.settingsGet();
      setSettings(s);
      applyAppearance(s);
    } catch (e) {
      toast(errText(e), "error");
    }
  };
  const refreshSync = async () => {
    try {
      const s = await ipc.syncStatus();
      setSync(s);
      setPhase(s.phase);
      if (s.balance_uhash) setBalance(s.balance_uhash);
    } catch {
      /* locked or not up yet */
    }
  };
  const refreshCounts = async () => {
    if (locked()) return;
    try {
      const c = await ipc.mailCounts();
      setCounts(c);
    } catch {
      /* locked */
    }
    try {
      const r = await ipc.peopleList("incoming");
      setRequestsIn(r.length);
    } catch {
      /* locked */
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
  const refreshIdentity = async () => {
    if (locked()) {
      setIdentity(null);
      return null;
    }
    try {
      const value = await ipc.identityStatus();
      setIdentity(value);
      return value;
    } catch {
      // Keep the last known registration state during a short outage.
      return identity();
    }
  };

  const onSyncEvent = (ev: SyncEvent) => {
    switch (ev.kind) {
      case "phase":
        setPhase(ev.phase);
        break;
      case "new_mail":
        bump("mail");
        void refreshCounts();
        break;
      case "drive_share_changed":
        bump("drive", "mail");
        break;
      case "contacts_changed":
        bump("people", "feed");
        void refreshCounts();
        break;
      case "circle_activity":
        bump("circles", "feed");
        break;
      case "space_activity":
        bump("spaces");
        break;
      case "balance":
        setBalance(ev.uhash);
        if (
          !registrationReadyNotified &&
          identity()?.this_device_registered === false &&
          BigInt(ev.uhash || "0") > 0n
        ) {
          registrationReadyNotified = true;
          toast("HASH received — this PC is ready for identity registration.", "info", {
            label: "Finish setup",
            run: () => {
              window.location.hash = "/wallet/devices";
            },
          });
        }
        break;
      case "round_done":
        if (ev.mail || ev.drive || ev.people || ev.circles || ev.spaces || ev.feed || ev.drive_committed !== null) {
          const areas: Refresh[] = [];
          if (ev.mail) areas.push("mail");
          if (ev.drive || ev.drive_committed !== null) areas.push("drive");
          if (ev.people) areas.push("people");
          if (ev.circles) areas.push("circles");
          if (ev.spaces) areas.push("spaces");
          if (ev.feed) areas.push("feed");
          bump(...areas);
        }
        void refreshSync();
        break;
      case "warning":
        void ipc.uiLog("warn", "sync warning").catch(() => undefined);
        break;
      default:
        break;
    }
  };

  let wired = false;
  const wire = async () => {
    if (wired) return;
    wired = true;
    await on("session:locked", () => {
      setLocked(true);
      setCounts({});
      setRequestsIn(0);
      setIdentity(null);
      registrationReadyNotified = false;
      setPhase("Offline");
      void refreshStatus();
    });
    await on("session:unlocked", () => {
      setLocked(false);
      void refreshStatus();
      void refreshCounts();
      void refreshSync();
      void refreshIdentity();
    });
    await on("settings:changed", () => void refreshSettings());
    await on("net:changed", () => {
      void refreshStatus();
      void refreshSync();
      if (!identity() || !identity()!.online) void refreshIdentity();
      bump("network");
    });
    await on("sync:phase", (p) => setPhase(p));
    await on("sync:event", onSyncEvent);
    await on("sync:tick", () => void refreshSync());
    await on("tx:update", (p) => {
      void refreshPending();
      bump("wallet");
      if (p.state === "committed") {
        toast(`Transaction committed at height ${p.height ?? "?"}`);
        void refreshIdentity();
      }
      if (p.state === "failed") toast(`Transaction failed: ${p.raw_log ?? ""}`, "error");
    });
    window.matchMedia?.("(prefers-color-scheme: light)").addEventListener?.("change", () => applyAppearance(settings()));
  };

  return {
    status,
    settings,
    sync,
    phase,
    counts,
    requestsIn,
    toasts,
    locked,
    pendingTx,
    balance,
    identity,
    ticks,
    setLocked,
    toast,
    dismissToast,
    bump,
    refreshStatus,
    refreshSettings,
    refreshSync,
    refreshCounts,
    refreshPending,
    refreshIdentity,
    applyAppearance,
    wire,
  };
}

export const store = createRoot(createStore);

/** Phase helpers shared by the shell and screens. */
export function phaseKind(p: SyncPhase): "idle" | "syncing" | "connecting" | "offline" | "backoff" {
  if (p === "Idle") return "idle";
  if (p === "Connecting" || p === "Discovering") return "connecting";
  if (p === "Offline") return "offline";
  if (typeof p === "object" && "Syncing" in p) return "syncing";
  return "backoff";
}

export function isOffline(p: SyncPhase): boolean {
  const k = phaseKind(p);
  return k === "offline" || k === "backoff" || k === "connecting";
}
