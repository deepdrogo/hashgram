// The application frame: left rail, content, status bar. Keyboard-first:
// every rail item has an accelerator, Ctrl+K opens search, Ctrl+L locks,
// Ctrl+Shift+P opens the performance panel.
import { For, Show, createEffect, createSignal, onMount, onCleanup, type ParentProps } from "solid-js";
import { A, useLocation, useNavigate } from "@solidjs/router";
import {
  Home,
  MessageSquare,
  Rss,
  Film,
  Radio,
  Phone,
  Wallet,
  Coins,
  Landmark,
  PieChart,
  Network,
  Settings,
  CircleHelp,
  Search,
  Lock,
  Download,
  X,
} from "lucide-solid";
import { store } from "~/lib/store";
import { updates } from "~/lib/updates";
import { ipc } from "~/lib/ipc";
import { verificationLabel } from "~/lib/format";
import { HealthDot } from "./identity";
import { CommandPalette } from "./CommandPalette";
import { PerfPanel } from "./PerfPanel";
import { Kbd } from "./ui";

const NAV = [
  { to: "/", label: "Home", icon: Home, key: "1" },
  { to: "/messages", label: "Messages", icon: MessageSquare, key: "2" },
  { to: "/feed", label: "Feed", icon: Rss, key: "3" },
  { to: "/reels", label: "Reels", icon: Film, key: "4" },
  { to: "/channels", label: "Channels", icon: Radio, key: "5" },
  { to: "/calls", label: "Calls", icon: Phone, key: "6" },
  { to: "/wallet", label: "Wallet", icon: Wallet, key: "7" },
  { to: "/earn", label: "Earn", icon: Coins, key: "8" },
  { to: "/founder", label: "Founder", icon: Landmark, key: "" },
  { to: "/supply", label: "Supply", icon: PieChart, key: "" },
  { to: "/network", label: "Network", icon: Network, key: "9" },
  { to: "/settings", label: "Settings", icon: Settings, key: "0" },
];

export function Shell(props: ParentProps) {
  const [palette, setPalette] = createSignal(false);
  const [perf, setPerf] = createSignal(false);
  const navigate = useNavigate();
  const location = useLocation();

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((v) => !v);
      } else if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "p") {
        e.preventDefault();
        setPerf((v) => !v);
      } else if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === "l") {
        e.preventDefault();
        void ipc.lock();
      } else if (e.altKey && /^[0-9]$/.test(e.key)) {
        const item = NAV.find((n) => n.key === e.key);
        if (item) {
          e.preventDefault();
          navigate(item.to);
        }
      } else if (e.key === "F1") {
        e.preventDefault();
        navigate("/help");
      }
    };
    window.addEventListener("keydown", onKey);
    const activity = () => void ipc.touch();
    window.addEventListener("pointerdown", activity);
    window.addEventListener("keydown", activity);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", activity);
      window.removeEventListener("keydown", activity);
    });
  });

  // Automatic update check, once, after the session is open and settings are known.
  createEffect(() => {
    if (!store.locked() && store.settings() && store.status()) void updates.checkOnStart();
  });
  const updateBanner = () => {
    const p = updates.phase();
    return (p === "available" && !updates.dismissed()) || p === "downloading" || p === "installing";
  };

  const net = store.net;
  const health = store.health;
  const nodesState = () => {
    const n = net();
    if (!n?.running) return "off" as const;
    if (n.verified === 0) return "bad" as const;
    if (n.verified === 1) return "warn" as const;
    return "ok" as const;
  };
  const chainState = () => {
    const h = health();
    if (!h?.active) return "off" as const;
    const v = net()?.last_read;
    if (h.active.kind === "p2p_relay" && v && !v.agreed) return "warn" as const;
    return "ok" as const;
  };
  const isActive = (to: string) => (to === "/" ? location.pathname === "/" : location.pathname.startsWith(to));

  return (
    <div class="flex h-full flex-col">
      <Show when={store.status()?.network === "devnet"}>
        <div class="flex h-7 shrink-0 items-center justify-center border-b border-fg bg-fg text-xs font-semibold tracking-wide text-bg" role="status">
          DEVNET — this is not Hashgram Mainnet. Nothing here has value.
        </div>
      </Show>
      <Show when={updateBanner()}>
        <div class="flex h-8 shrink-0 items-center gap-3 border-b border-border bg-surface px-3 text-xs" role="status">
          <Download size={14} aria-hidden="true" />
          <span class="flex-1">
            <Show when={updates.phase() === "available"}>Hashgram {updates.available()?.version} is available.</Show>
            <Show when={updates.phase() === "downloading"}>Downloading Hashgram {updates.available()?.version}…</Show>
            <Show when={updates.phase() === "installing"}>Signature verified — installing and restarting…</Show>
          </span>
          <Show when={updates.phase() === "available"}>
            <button type="button" class="btn-primary h-6 px-2.5 text-xs" onClick={() => void updates.install()}>
              Install and restart
            </button>
            <A href="/settings" class="text-muted hover:text-fg">
              Details
            </A>
            <button type="button" class="text-muted hover:text-fg" aria-label="Dismiss" onClick={() => updates.setDismissed(true)}>
              <X size={14} />
            </button>
          </Show>
        </div>
      </Show>
      <div class="flex min-h-0 flex-1">
        <nav class="flex w-[212px] shrink-0 flex-col border-r border-border bg-surface" aria-label="Main">
          <div class="flex h-12 items-center gap-2 px-4">
            <svg viewBox="0 0 64 64" width="18" height="18" aria-hidden="true">
              <rect x="21" y="12" width="6" height="40" fill="#ffffff" />
              <rect x="37" y="12" width="6" height="40" fill="#ffffff" />
              <rect x="12" y="21" width="40" height="6" fill="#ffffff" />
              <rect x="12" y="37" width="40" height="6" fill="#ffffff" />
            </svg>
            <span class="text-sm font-semibold tracking-tight">Hashgram</span>
          </div>
          <button
            type="button"
            class="mx-3 mb-2 flex h-8 items-center gap-2 rounded-md border border-border bg-surface-2 px-2.5 text-xs text-muted hover:text-fg"
            onClick={() => setPalette(true)}
          >
            <Search size={14} />
            <span class="flex-1 text-left">Search</span>
            <Kbd>Ctrl K</Kbd>
          </button>
          <ul class="flex-1 space-y-0.5 px-2">
            <For each={NAV}>
              {(item) => (
                <li>
                  <A
                    href={item.to}
                    class={`flex h-9 items-center gap-3 rounded-md px-2.5 text-sm transition-colors ${
                      isActive(item.to) ? "bg-surface-2 text-fg" : "text-muted hover:bg-surface-2 hover:text-fg"
                    }`}
                    aria-current={isActive(item.to) ? "page" : undefined}
                    title={`${item.label} (Alt+${item.key})`}
                  >
                    <item.icon size={16} aria-hidden="true" />
                    <span class="flex-1">{item.label}</span>
                    <Show when={item.to === "/wallet" && store.pendingTx() > 0}>
                      <span class="badge-strong" title="pending transactions">
                        {store.pendingTx()}
                      </span>
                    </Show>
                  </A>
                </li>
              )}
            </For>
          </ul>
          <div class="space-y-0.5 px-2 pb-2">
            <A href="/help" class={`flex h-9 items-center gap-3 rounded-md px-2.5 text-sm ${isActive("/help") ? "bg-surface-2 text-fg" : "text-muted hover:bg-surface-2 hover:text-fg"}`}>
              <CircleHelp size={16} aria-hidden="true" />
              <span class="flex-1">Help</span>
              <Kbd>F1</Kbd>
            </A>
            <button type="button" class="flex h-9 w-full items-center gap-3 rounded-md px-2.5 text-sm text-muted hover:bg-surface-2 hover:text-fg" onClick={() => void ipc.lock()}>
              <Lock size={16} aria-hidden="true" />
              <span class="flex-1 text-left">Lock</span>
              <Kbd>Ctrl L</Kbd>
            </button>
          </div>
        </nav>
        <main class="min-w-0 flex-1 overflow-auto" id="main">
          {props.children}
        </main>
      </div>
      <footer class="flex h-7 shrink-0 items-center gap-4 border-t border-border bg-surface px-3 text-xs text-muted" role="contentinfo">
        <A href="/network" class="flex items-center gap-1.5 hover:text-fg" title="Connected nodes">
          <HealthDot state={nodesState()} />
          <span>
            {net()?.verified ?? 0} node{(net()?.verified ?? 0) === 1 ? "" : "s"}
          </span>
        </A>
        <A href="/network" class="flex items-center gap-1.5 hover:text-fg" title="Chain source">
          <HealthDot state={chainState()} />
          <span>
            {health()?.active
              ? verificationLabel(net()?.last_read, health()?.active)
              : "no chain source"}
          </span>
        </A>
        <Show when={store.pendingTx() > 0}>
          <span>{store.pendingTx()} pending</span>
        </Show>
        <span class="flex-1" />
        <span class="mono">{store.status()?.chain_id}</span>
        <span class="mono" title={store.status()?.commit}>
          v{store.status()?.version}
        </span>
      </footer>
      <CommandPalette open={palette()} onClose={() => setPalette(false)} />
      <PerfPanel open={perf()} onClose={() => setPerf(false)} />
      <Toasts />
    </div>
  );
}

function Toasts() {
  return (
    <div class="pointer-events-none fixed bottom-9 right-3 z-50 flex flex-col gap-2" aria-live="polite">
      <For each={store.toasts()}>
        {(t) => (
          <div class={`pointer-events-auto card px-3 py-2 text-xs fade-in ${t.kind === "error" ? "border-fg" : ""}`} role="status">
            {t.text}
          </div>
        )}
      </For>
    </div>
  );
}
