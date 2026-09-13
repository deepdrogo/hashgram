// The application frame: left rail (Mail · Drive · Feed · People · Spaces ·
// Earn · Wallet · Network · Settings), top bar with search and sync state,
// toasts. Keyboard: Ctrl+K search, Ctrl+L lock, Alt+1..9 sections.
import { For, Show, createEffect, createSignal, onMount, onCleanup, type ParentProps } from "solid-js";
import { A, useLocation, useNavigate } from "@solidjs/router";
import { Mail, HardDrive, Rss, Users, LayoutGrid, Coins, Wallet, Network, Settings, CircleHelp, Search, Lock, Download, X, ShieldAlert, LogOut, Compass, UserCircle } from "lucide-solid";
import { store } from "~/lib/store";
import { updates } from "~/lib/updates";
import { ipc } from "~/lib/ipc";
import { t, type Key } from "~/lib/i18n";
import { CommandPalette } from "./CommandPalette";
import { PerfPanel } from "./PerfPanel";
import { SyncIndicator } from "./SyncIndicator";
import { Kbd, Button } from "./ui";
import { confirm } from "~/lib/dialogs";

export const NAV: { to: string; key: Key; icon: typeof Mail; accel: string }[] = [
  { to: "/mail", key: "nav_mail", icon: Mail, accel: "1" },
  { to: "/drive", key: "nav_drive", icon: HardDrive, accel: "2" },
  { to: "/hashwall", key: "nav_feed", icon: Rss, accel: "3" },
  { to: "/explore", key: "nav_explore", icon: Compass, accel: "4" },
  { to: "/people", key: "nav_people", icon: Users, accel: "5" },
  { to: "/spaces", key: "nav_spaces", icon: LayoutGrid, accel: "6" },
  { to: "/earn", key: "nav_earn", icon: Coins, accel: "7" },
  { to: "/wallet", key: "nav_wallet", icon: Wallet, accel: "8" },
  { to: "/network", key: "nav_network", icon: Network, accel: "9" },
  { to: "/me", key: "nav_me", icon: UserCircle, accel: "0" },
  { to: "/settings", key: "nav_settings", icon: Settings, accel: "" },
];

export function Shell(props: ParentProps) {
  const [palette, setPalette] = createSignal(false);
  const [perf, setPerf] = createSignal(false);
  const navigate = useNavigate();
  const location = useLocation();
  const signOut = async () => {
    const ok = await confirm(
      "Sign out and remove this account from this PC?\n\nThis deletes the local vault, mail and files. Make sure you have the 24 words or an encrypted backup. This cannot be undone.",
    );
    if (!ok) return;
    await ipc.lock();
    await ipc.wipeLocalData("DELETE");
    window.location.reload();
  };

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
        const item = NAV.find((n) => n.accel && n.accel === e.key);
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
    let last = 0;
    const activity = () => {
      const now = Date.now();
      if (now - last > 15_000) {
        last = now;
        void ipc.touch().catch(() => undefined);
      }
    };
    window.addEventListener("pointerdown", activity);
    window.addEventListener("keydown", activity);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", activity);
      window.removeEventListener("keydown", activity);
    });
  });

  createEffect(() => {
    if (!store.locked() && store.settings() && store.status()) void updates.checkOnStart();
  });
  const updateBanner = () => {
    const p = updates.phase();
    return (p === "available" && !updates.dismissed()) || p === "downloading" || p === "installing";
  };

  const isActive = (to: string) => location.pathname.startsWith(to);
  const badge = (to: string): number => {
    if (to === "/mail") return (store.counts()["inbox"]?.unread ?? 0) + (store.counts()["requests"]?.total ?? 0);
    if (to === "/people") return store.requestsIn();
    if (to === "/wallet") return store.pendingTx();
    return 0;
  };

  return (
    <div class="flex h-full flex-col">
      <Show when={store.status()?.network === "devnet"}>
        <div class="flex h-6 shrink-0 items-center justify-center border-b border-fg bg-fg text-[11px] font-semibold tracking-wide text-bg" role="status">
          {t("devnet_banner")}
        </div>
      </Show>
      <Show when={updateBanner()}>
        <div class="flex h-8 shrink-0 items-center gap-3 border-b border-border bg-surface px-3 text-xs" role="status">
          <Download size={14} aria-hidden="true" />
          <span class="flex-1">
            <Show when={updates.phase() === "available"}>Hashgram One {updates.available()?.version} is available.</Show>
            <Show when={updates.phase() === "downloading"}>Downloading Hashgram One {updates.available()?.version}…</Show>
            <Show when={updates.phase() === "installing"}>Signature verified — installing and restarting…</Show>
          </span>
          <Show when={updates.phase() === "available"}>
            <Button variant="brand" size="sm" onClick={() => void updates.install()}>
              Install and restart
            </Button>
            <A href="/settings/updates" class="text-muted hover:text-fg">
              Details
            </A>
            <button type="button" class="text-muted hover:text-fg" aria-label="Dismiss" onClick={() => updates.setDismissed(true)}>
              <X size={14} />
            </button>
          </Show>
        </div>
      </Show>
      <div class="flex min-h-0 flex-1">
        <nav class="flex w-[196px] shrink-0 flex-col border-r border-border bg-surface" aria-label="Main">
          <div class="flex h-11 items-center gap-2 px-3.5">
            <Logo size={16} />
            <span class="text-[13px] font-semibold tracking-tight">{t("app_name")}</span>
          </div>
          <ul class="flex-1 space-y-px px-2 pt-1">
            <For each={NAV}>
              {(item) => (
                <li>
                  <A
                    href={item.to}
                    class={`row flex h-8 items-center gap-2.5 rounded-md px-2.5 text-[13px] ${isActive(item.to) ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`}
                    aria-current={isActive(item.to) ? "page" : undefined}
                    title={item.accel ? `${t(item.key)} (Alt+${item.accel})` : t(item.key)}
                    data-nav={item.key}
                  >
                    <item.icon size={15} aria-hidden="true" class={isActive(item.to) ? "text-brand" : ""} />
                    <span class="flex-1">{t(item.key)}</span>
                    <Show when={badge(item.to) > 0}>
                      <span class="badge-strong tnum" title={String(badge(item.to))}>
                        {badge(item.to) > 99 ? "99+" : badge(item.to)}
                      </span>
                    </Show>
                  </A>
                </li>
              )}
            </For>
          </ul>
          <div class="space-y-px px-2 pb-2">
            <A href="/help" class={`row flex h-8 items-center gap-2.5 rounded-md px-2.5 text-[13px] ${isActive("/help") ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`}>
              <CircleHelp size={15} aria-hidden="true" />
              <span class="flex-1">{t("nav_help")}</span>
              <Kbd>F1</Kbd>
            </A>
            <button type="button" class="row flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-[13px] text-muted hover:text-fg" onClick={() => void ipc.lock()}>
              <Lock size={15} aria-hidden="true" />
              <span class="flex-1 text-left">{t("lock")}</span>
              <Kbd>Ctrl L</Kbd>
            </button>
            <button type="button" class="row flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-[13px] text-muted hover:text-fg" onClick={() => void signOut()}>
              <LogOut size={15} aria-hidden="true" />
              <span class="flex-1 text-left">Sign out</span>
            </button>
          </div>
        </nav>
        <div class="flex min-w-0 flex-1 flex-col">
          <header class="flex h-11 shrink-0 items-center gap-2 border-b border-border bg-surface px-3">
            <button
              type="button"
              class="flex h-7 w-full max-w-xl items-center gap-2 rounded-md border border-border bg-surface-2 px-2.5 text-xs text-muted hover:text-fg"
              onClick={() => setPalette(true)}
            >
              <Search size={13} />
              <span class="flex-1 truncate text-left">{t("search_placeholder")}</span>
              <Kbd>Ctrl K</Kbd>
            </button>
            <span class="flex-1" />
            <SyncIndicator />
            <button type="button" class="btn-ghost btn-icon-sm" title={`${t("lock")} (Ctrl+L)`} aria-label={t("lock")} onClick={() => void ipc.lock()}>
              <Lock size={14} />
            </button>
          </header>
          <Show when={store.identity() && !store.identity()!.this_device_registered}>
            <div class="flex min-h-10 shrink-0 items-center gap-3 border-b border-border bg-surface-2 px-3 text-xs" role="alert">
              <ShieldAlert size={15} class="text-brand" aria-hidden="true" />
              <span class="flex-1">
                This PC is not registered for your identity. Posting and mail need at least 0.01 HASH for the one-time registration; a username additionally costs 1 HASH.
              </span>
              <Button size="sm" variant="brand" onClick={() => navigate("/wallet/devices")}>
                Finish setup
              </Button>
            </div>
          </Show>
          <main class="min-h-0 min-w-0 flex-1 overflow-hidden" id="main">
            {props.children}
          </main>
        </div>
      </div>
      <CommandPalette open={palette()} onClose={() => setPalette(false)} />
      <PerfPanel open={perf()} onClose={() => setPerf(false)} />
      <Toasts />
    </div>
  );
}

export function Logo(props: { size?: number }) {
  const s = () => props.size ?? 18;
  return (
    <svg viewBox="0 0 64 64" width={s()} height={s()} aria-hidden="true" class="text-fg">
      <rect x="21" y="12" width="6" height="40" fill="currentColor" />
      <rect x="37" y="12" width="6" height="40" fill="currentColor" />
      <rect x="12" y="21" width="40" height="6" fill="currentColor" />
      <rect x="12" y="37" width="40" height="6" fill="currentColor" />
    </svg>
  );
}

function Toasts() {
  return (
    <div class="pointer-events-none fixed bottom-3 right-3 z-50 flex flex-col gap-2" aria-live="polite">
      <For each={store.toasts()}>
        {(tst) => (
          <div class={`pointer-events-auto card flex items-center gap-3 px-3 py-2 text-xs fade-in ${tst.kind === "error" ? "border-fg" : ""}`} role="status">
            <span class="max-w-sm selectable">{tst.text}</span>
            <Show when={tst.action}>
              <button
                type="button"
                class="font-medium text-brand hover:underline"
                onClick={() => {
                  tst.action?.run();
                  store.dismissToast(tst.id);
                }}
              >
                {tst.action?.label}
              </button>
            </Show>
            <button type="button" class="text-muted hover:text-fg" aria-label="Dismiss" onClick={() => store.dismissToast(tst.id)}>
              <X size={12} />
            </button>
          </div>
        )}
      </For>
    </div>
  );
}
