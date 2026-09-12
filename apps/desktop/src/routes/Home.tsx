import { createResource, Show, For } from "solid-js";
import { A } from "@solidjs/router";
import { Card, Stat, Skeleton } from "~/components/ui";
import { Mono, VerifiedBy, PersonLabel } from "~/components/identity";
import { ipc, pick, str, num } from "~/lib/ipc";
import { formatHash, formatUhash, relTime, verificationLabel } from "~/lib/format";
import { store } from "~/lib/store";

export function Home() {
  const [wallet] = createResource(async () => {
    try {
      return { ok: await ipc.walletOverview(), error: null as string | null };
    } catch (e) {
      return { ok: null, error: String(e) };
    }
  });
  const [epoch] = createResource(() => ipc.chainGet("hashgram/serviceproof/v1/epoch/current").catch(() => null));
  const [recent] = createResource(() => ipc.txRecent().catch(() => []));
  const [convs] = createResource(() => ipc.chatList().catch(() => []));
  const [feed] = createResource(() => ipc.feed(undefined, undefined, undefined, 10).catch(() => ({ events: [], hidden: 0, authors: [] })));
  const net = store.net;

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-baseline justify-between">
        <h1 class="page-title">Home</h1>
        <Show when={store.status()?.address}>
          <Mono text={store.status()!.address!} copy class="text-xs text-muted" />
        </Show>
      </div>
      <div class="grid grid-cols-3 gap-3">
        <Show when={wallet()} fallback={<div class="stat"><Skeleton lines={2} /></div>}>
          {(w) => (
            <Show
              when={w().ok}
              fallback={
                <div class="stat">
                  <span class="stat-label">Balance</span>
                  <span class="text-xs text-muted">{w().error}</span>
                </div>
              }
            >
              {(o) => (
                <Stat
                  label="Balance"
                  value={<span title={formatUhash(o().balance_uhash)}>{formatHash(o().balance_uhash)} HASH</span>}
                  sub={<VerifiedBy verification={o().verification} source={o().source} />}
                />
              )}
            </Show>
          )}
        </Show>
        <Stat
          label="Network"
          mono={false}
          value={`${net()?.verified ?? 0} nodes`}
          sub={net()?.last_read ? verificationLabel(net()!.last_read) : "no chain read yet"}
        />
        <Stat
          label="Current epoch"
          value={epoch() ? str(pick(epoch()!.value, "epoch.number"), "—") : "—"}
          sub={epoch() ? `blocks ${str(pick(epoch()!.value, "epoch.start_height"))}–${str(pick(epoch()!.value, "epoch.end_height"))}` : "waiting for a chain source"}
        />
      </div>
      <div class="grid grid-cols-2 gap-3">
        <Card title="Recent transactions" actions={<A href="/wallet/history" class="text-xs text-muted hover:text-fg">All</A>}>
          <Show when={recent()} fallback={<div class="p-4"><Skeleton lines={3} /></div>}>
            <Show when={recent()!.length} fallback={<p class="p-4 text-xs text-muted">Nothing sent from this PC yet.</p>}>
              <ul>
                <For each={recent()!.slice(0, 6)}>
                  {(r) => (
                    <li class="flex items-center justify-between gap-3 border-b border-border px-4 py-2 text-sm last:border-0">
                      <span class="truncate">{r.summary}</span>
                      <span class="mono shrink-0 text-xs text-muted">
                        {r.state}
                        {r.height ? ` · ${num(r.height)}` : ""} · {relTime(r.submitted)}
                      </span>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Card>
        <Card title="Last messages" actions={<A href="/messages" class="text-xs text-muted hover:text-fg">All</A>}>
          <Show when={convs()} fallback={<div class="p-4"><Skeleton lines={3} /></div>}>
            <Show when={convs()!.length} fallback={<p class="p-4 text-xs text-muted">No conversations yet. Messages are end-to-end encrypted (MLS) and fetched from store nodes.</p>}>
              <ul>
                <For each={convs()!.slice(0, 6)}>
                  {(c) => (
                    <li>
                      <A href={`/messages/${c.group_id}`} class="row-hover flex items-center justify-between gap-3 border-b border-border px-4 py-2 text-sm last:border-0">
                        <span class="min-w-0 truncate">
                          <Show when={c.direct} fallback={<span class="font-medium">{c.name}</span>}>
                            <PersonLabel person={{ address: c.members.find((m) => m !== store.status()?.address) ?? "" }} size="sm" />
                          </Show>
                          <span class="ml-2 text-xs text-muted">{c.last_preview}</span>
                        </span>
                        <Show when={c.unread}>
                          <span class="badge-strong">{c.unread}</span>
                        </Show>
                      </A>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Card>
      </div>
      <div class="grid grid-cols-1 gap-3">
        <Card title="Feed highlights" actions={<A href="/feed" class="text-xs text-muted hover:text-fg">Open feed</A>}>
          <Show when={feed()} fallback={<div class="p-4"><Skeleton lines={3} /></div>}>
            <Show when={feed()!.events.length} fallback={<p class="p-4 text-xs text-muted">Follow people to see their posts here. Nothing is ranked; there is no server to rank it.</p>}>
              <ul>
                <For each={feed()!.events.slice(0, 5)}>
                  {(e) => (
                    <li class="flex items-baseline gap-3 border-b border-border px-4 py-2 text-sm last:border-0">
                      <PersonLabel person={{ address: e.author }} size="sm" />
                      <span class="min-w-0 flex-1 truncate">{str(pick(e.payload, "text")) || str(pick(e.payload, "caption"))}</span>
                      <span class="mono shrink-0 text-xs text-muted">{relTime(e.timestamp)}</span>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Card>
      </div>
    </div>
  );
}
