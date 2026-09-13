// Shared empty / loading / error / offline states, so every screen tells
// the same truth the same way.
import { Show, createEffect, createSignal, onCleanup, type JSX, type ParentProps } from "solid-js";
import { WifiOff, AlertTriangle, RefreshCw } from "lucide-solid";
import { Button, Skeleton } from "./ui";
import { store, isOffline } from "~/lib/store";
import { t } from "~/lib/i18n";
import { errText, errCode } from "~/lib/ipc";

export function Loading(props: { lines?: number; class?: string }) {
  return (
    <div class={`p-4 ${props.class ?? ""}`} aria-busy="true">
      <Skeleton lines={props.lines ?? 4} />
    </div>
  );
}

export function ErrorState(props: { error: unknown; onRetry?: () => void; class?: string; compact?: boolean }) {
  const code = () => errCode(props.error);
  const offline = () => code() === "offline";
  return (
    <div class={`flex flex-col items-center justify-center gap-2 px-6 ${props.compact ? "py-4" : "py-10"} text-center ${props.class ?? ""}`} role="alert">
      <Show when={offline()} fallback={<AlertTriangle size={18} class="text-muted" />}>
        <WifiOff size={18} class="text-muted" />
      </Show>
      <p class="text-sm font-medium">{offline() ? t("sync_offline") : t("error_title")}</p>
      <p class="max-w-md text-xs text-muted selectable">{errText(props.error)}</p>
      <Show when={props.onRetry}>
        <Button variant="secondary" size="sm" onClick={props.onRetry}>
          <RefreshCw size={12} />
          {t("retry")}
        </Button>
      </Show>
    </div>
  );
}

/** Wraps a resource: skeleton while loading, error with retry, else children. */
export function Resource<T>(props: {
  value: () => T | undefined;
  loading: boolean;
  error?: unknown;
  onRetry?: () => void;
  lines?: number;
  children: (v: T) => JSX.Element;
  empty?: JSX.Element;
  isEmpty?: (v: T) => boolean;
}) {
  const ready = () => props.value() !== undefined;
  const emptyNow = () => {
    const v = props.value();
    return v !== undefined && (props.isEmpty?.(v) ?? false);
  };
  return (
    <Show when={!props.error} fallback={<ErrorState error={props.error} onRetry={props.onRetry} />}>
      <Show when={ready()} fallback={props.loading ? <Loading lines={props.lines} /> : null}>
        <Show when={!emptyNow()} fallback={props.empty}>
          {props.children(props.value() as T)}
        </Show>
      </Show>
    </Show>
  );
}

/** How long the link must stay peerless before the banner appears. A
 *  redial after a dropped connection takes a few seconds; showing
 *  "offline" for each one made the app look broken when it was not. */
export const OFFLINE_GRACE_MS = 8_000;

/** True once `offline` has been continuously true for `graceMs`. */
export function useSustained(offline: () => boolean, graceMs = OFFLINE_GRACE_MS): () => boolean {
  const [sustained, setSustained] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    if (offline()) {
      if (timer === undefined && !sustained()) {
        timer = setTimeout(() => {
          timer = undefined;
          setSustained(true);
        }, graceMs);
      }
    } else {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      setSustained(false);
    }
  });
  onCleanup(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
  return sustained;
}

/** The thin banner every screen shows when there has been no peer for a while. */
export function OfflineBanner() {
  const sustained = useSustained(() => !store.locked() && isOffline(store.phase()));
  return (
    <Show when={sustained()}>
      <div class="flex h-7 shrink-0 items-center gap-2 border-b border-border bg-surface px-3 text-xs text-muted" role="status">
        <WifiOff size={12} />
        <span>{t("offline_banner")}</span>
      </div>
    </Show>
  );
}

export function Toolbar(props: ParentProps<{ class?: string }>) {
  return <div class={`flex h-10 shrink-0 items-center gap-1.5 border-b border-border px-2 ${props.class ?? ""}`}>{props.children}</div>;
}

export function SectionTitle(props: ParentProps<{ class?: string }>) {
  return <h3 class={`px-3 pt-3 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted ${props.class ?? ""}`}>{props.children}</h3>;
}
