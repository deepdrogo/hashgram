// Ctrl+K search. Resolves, in order: hash1… address → @username (chain
// lookup) → transaction hash → #hashtag → channel. There is no fuzzy
// "people you may know": no server exists to compute one.
import { createSignal, createEffect, Show, For, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { useNavigate } from "@solidjs/router";
import { Search, User, AtSign, Hash, Receipt, Radio, CircleAlert } from "lucide-solid";
import { ipc, type SearchResult } from "~/lib/ipc";
import { truncateMiddle } from "~/lib/format";
import { Kbd } from "./ui";

export function CommandPalette(props: { open: boolean; onClose: () => void }) {
  const [q, setQ] = createSignal("");
  const [result, setResult] = createSignal<SearchResult | null>(null);
  const [recent, setRecent] = createSignal<string[]>([]);
  const [busy, setBusy] = createSignal(false);
  const navigate = useNavigate();
  let input!: HTMLInputElement;
  let timer: ReturnType<typeof setTimeout> | null = null;

  createEffect(() => {
    if (props.open) {
      setQ("");
      setResult(null);
      void ipc.searchRecent().then(setRecent).catch(() => undefined);
      queueMicrotask(() => input?.focus());
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "Escape") props.onClose();
      };
      window.addEventListener("keydown", onKey);
      onCleanup(() => window.removeEventListener("keydown", onKey));
    }
  });

  const run = (value: string) => {
    if (timer) clearTimeout(timer);
    if (!value.trim()) {
      setResult(null);
      return;
    }
    timer = setTimeout(async () => {
      setBusy(true);
      try {
        setResult(await ipc.searchResolve(value));
      } catch (e) {
        setResult({ kind: "nothing", reason: String(e) });
      } finally {
        setBusy(false);
      }
    }, 180);
  };

  const go = (r: SearchResult) => {
    switch (r.kind) {
      case "address":
        navigate(`/profile/${r.address}`);
        break;
      case "username":
        navigate(`/profile/${r.address}`);
        break;
      case "username_available":
        navigate(`/wallet/usernames?register=${encodeURIComponent(r.name)}`);
        break;
      case "tx":
        navigate(`/wallet/history?tx=${r.hash}`);
        break;
      case "hashtag":
        navigate(`/feed?tag=${encodeURIComponent(r.tag)}`);
        break;
      case "channel":
        navigate(`/channels/${encodeURIComponent(r.id)}`);
        break;
      default:
        return;
    }
    props.onClose();
  };

  const Row = (p: { icon: typeof Search; title: string; sub?: string; onClick?: () => void; disabled?: boolean }) => (
    <button
      type="button"
      class="row-hover flex w-full items-center gap-3 rounded-md px-3 py-2 text-left disabled:opacity-60"
      disabled={p.disabled}
      onClick={p.onClick}
    >
      <p.icon size={16} class="shrink-0 text-muted" aria-hidden="true" />
      <span class="min-w-0 flex-1">
        <span class="block truncate text-sm">{p.title}</span>
        <Show when={p.sub}>
          <span class="mono block truncate text-xs text-muted">{p.sub}</span>
        </Show>
      </span>
    </button>
  );

  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-start justify-center bg-bg/80 pt-[12vh] fade-in" onClick={props.onClose} role="presentation">
          <div class="card w-full max-w-xl" role="dialog" aria-label="Search" onClick={(e) => e.stopPropagation()}>
            <div class="flex items-center gap-2 border-b border-border px-3">
              <Search size={16} class="text-muted" aria-hidden="true" />
              <input
                ref={input}
                class="h-11 flex-1 bg-transparent text-sm outline-none placeholder:text-muted"
                placeholder="hash1… address, @username, transaction hash, #hashtag, channel:name"
                value={q()}
                onInput={(e) => {
                  setQ(e.currentTarget.value);
                  run(e.currentTarget.value);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && result()) go(result()!);
                }}
                spellcheck={false}
                aria-label="Search"
              />
              <Kbd>Esc</Kbd>
            </div>
            <div class="max-h-[50vh] overflow-auto p-1.5">
              <Show when={busy()}>
                <p class="px-3 py-2 text-xs text-muted">Resolving on chain…</p>
              </Show>
              <Show when={result()}>
                {(r) => {
                  const v = r();
                  switch (v.kind) {
                    case "address":
                      return <Row icon={User} title={v.username ? `@${v.username}` : "Address"} sub={v.address} onClick={() => go(v)} />;
                    case "username":
                      return <Row icon={AtSign} title={`@${v.name}`} sub={v.address} onClick={() => go(v)} />;
                    case "username_available":
                      return (
                        <>
                          <Row icon={AtSign} title={`@${v.name} is available — register it`} sub="1 HASH, about a year" onClick={() => go(v)} />
                          <Show when={v.confusable_with.length}>
                            <p class="px-3 py-1 text-xs text-muted">Looks like: {v.confusable_with.map((c) => `@${c}`).join(", ")}</p>
                          </Show>
                        </>
                      );
                    case "tx":
                      return <Row icon={Receipt} title={v.found ? `Transaction at height ${v.height ?? "?"}` : "Transaction not found on chain"} sub={truncateMiddle(v.hash, 16, 8)} onClick={() => go(v)} disabled={!v.found} />;
                    case "hashtag":
                      return <Row icon={Hash} title={`#${v.tag}`} onClick={() => go(v)} />;
                    case "channel":
                      return <Row icon={Radio} title={`Channel ${v.id}`} onClick={() => go(v)} />;
                    default:
                      return <Row icon={CircleAlert} title={v.reason} disabled />;
                  }
                }}
              </Show>
              <Show when={!q() && recent().length}>
                <p class="px-3 pt-2 pb-1 text-[11px] uppercase tracking-wide text-muted">Recent</p>
                <For each={recent()}>
                  {(r) => (
                    <Row
                      icon={Search}
                      title={r}
                      onClick={() => {
                        setQ(r);
                        run(r);
                      }}
                    />
                  )}
                </For>
              </Show>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  );
}
