// The dot in the top bar: phase-driven, with a tooltip that says what is
// true right now. Click runs a round.
import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { RefreshCw } from "lucide-solid";
import { store, phaseKind } from "~/lib/store";
import { t } from "~/lib/i18n";
import { ipc } from "~/lib/ipc";
import { Dot } from "./identity";

export function SyncIndicator() {
  const [now, setNow] = createSignal(Date.now());
  onMount(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(id));
  });
  const kind = createMemo(() => phaseKind(store.phase()));
  const label = createMemo(() => {
    const p = store.phase();
    const s = store.sync();
    switch (kind()) {
      case "idle": {
        const ago = s?.last_ok_ms ? Math.max(0, Math.round((now() - s.last_ok_ms) / 1000)) : null;
        return ago === null ? t("sync_idle", { ago: "" }).trim() : t("sync_idle", { ago: ago < 5 ? "just now" : `${ago} s ago` });
      }
      case "syncing": {
        const stage = typeof p === "object" && "Syncing" in p ? p.Syncing : "";
        return `${t("sync_syncing")} ${stage}`.trim();
      }
      case "connecting":
        return p === "Discovering" ? t("sync_discovering") : t("sync_connecting");
      case "backoff": {
        const w = typeof p === "object" && "Backoff" in p ? p.Backoff.wait_secs : 0;
        return `${t("sync_offline")} · ${t("sync_backoff", { secs: w })}`;
      }
      default:
        return t("sync_offline");
    }
  });
  const dot = () => (kind() === "idle" ? "ok" : kind() === "syncing" ? "ok" : kind() === "connecting" ? "warn" : "bad") as "ok" | "warn" | "bad";
  const peers = () => store.sync()?.peers ?? 0;
  return (
    <button
      type="button"
      class="flex h-7 items-center gap-2 rounded-md px-2 text-xs text-muted hover:bg-surface-2 hover:text-fg"
      title={`${label()}\n${peers()} node${peers() === 1 ? "" : "s"}${store.sync()?.last_error ? `\n${store.sync()?.last_error}` : ""}\n${t("sync_now")}`}
      onClick={() => void ipc.syncNow().catch(() => undefined)}
      aria-label={label()}
    >
      <Dot state={dot()} class={kind() === "syncing" ? "animate-pulse" : ""} />
      <span class="max-w-[260px] truncate">{label()}</span>
      <Show when={kind() === "syncing"}>
        <RefreshCw size={11} class="animate-spin" />
      </Show>
    </button>
  );
}
