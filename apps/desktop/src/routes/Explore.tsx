// Explore: what the network is talking about, read from the nearest node
// one page at a time. Posts (optionally by hashtag), active walls, people
// (registered authors ranked by posts), hashtags, top HASH holders (read
// straight from the chain — no indexer needed) and providers.
import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Compass, Hash, Megaphone, Users, Crown, Server, RefreshCw, Search, X } from "lucide-solid";
import { Button, Input, Tabs, Badge, Empty, Select, Stat } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Who, PersonAvatar, Mono } from "~/components/identity";
import { ipc, type Holders, type ProviderStatus } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatHash, shortWhen } from "~/lib/format";
import { PostCard, WallCard, SourceLine, usePages } from "./feed/Feed";

type Section = "posts" | "walls" | "people" | "tags" | "holders" | "providers";
const SECTIONS: Section[] = ["posts", "walls", "people", "tags", "holders", "providers"];

const WINDOWS = [
  { value: String(24 * 3600), label: "Last 24 hours" },
  { value: String(7 * 24 * 3600), label: "Last 7 days" },
  { value: String(30 * 24 * 3600), label: "Last 30 days" },
];

// The rich list is expensive for the chain (paged account scan); keep it for
// the session and refresh only when asked.
let holdersCache: Holders | null = null;

export function ExploreRoute() {
  const params = useParams<{ section?: string; arg?: string }>();
  const navigate = useNavigate();
  const section = (): Section => (SECTIONS.includes(params.section as Section) ? (params.section as Section) : "posts");
  const [window, setWindow] = createSignal(String(7 * 24 * 3600));

  const [digest, { refetch: refetchDigest }] = createResource(
    () => (["walls", "people", "tags"].includes(section()) ? { w: Number(window()), tick: store.ticks().feed } : null),
    (k) => ipc.hashwallDigest(k.w, 50),
  );

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="flex items-center gap-2 border-b border-border px-3">
        <Tabs
          class="flex-1 border-b-0"
          value={section()}
          onChange={(v) => navigate(`/explore/${v}`)}
          tabs={[
            { id: "posts", label: t("explore_posts") },
            { id: "walls", label: t("explore_walls") },
            { id: "people", label: t("explore_people") },
            { id: "tags", label: "Hashtags" },
            { id: "holders", label: t("explore_holders") },
            { id: "providers", label: t("explore_providers") },
          ]}
        />
        <Show when={["walls", "people", "tags"].includes(section())}>
          <Select class="h-7" value={window()} onChange={setWindow} options={WINDOWS} aria-label="window" />
          <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => void refetchDigest()}>
            <RefreshCw size={13} />
          </Button>
        </Show>
      </div>
      <div class="min-h-0 flex-1 overflow-auto">
        <div class="mx-auto max-w-2xl px-4 py-3">
          <Show when={section() === "posts"}>
            <ExplorePosts tag={params.arg ? decodeURIComponent(params.arg) : ""} onTag={(tag) => navigate(tag ? `/explore/posts/${encodeURIComponent(tag)}` : "/explore/posts")} />
          </Show>
          <Show when={["walls", "people", "tags"].includes(section())}>
            <Show when={!digest.error} fallback={<ErrorState error={digest.error} onRetry={() => void refetchDigest()} />}>
              <Show when={digest()} fallback={<div class="flex flex-col gap-2"><div class="skeleton h-14 w-full" /><div class="skeleton h-14 w-full" /><div class="skeleton h-14 w-full" /></div>}>
                {(d) => (
                  <>
                    <SourceLine page={{ items: [], next_before: 0, source_peer: d().source_peer, source_operator: d().source_operator, source_rtt_ms: d().source_rtt_ms, note: d().note }} />
                    <div class="mb-3 grid grid-cols-2 gap-2 sm:grid-cols-4">
                      <Stat label="Posts in window" value={<span class="tnum">{d().events}</span>} />
                      <Stat label="People active" value={<span class="tnum">{d().authors}</span>} />
                      <Stat label="Posts held by node" value={<span class="tnum">{d().total_events}</span>} />
                      <Stat label="People ever seen" value={<span class="tnum">{d().total_authors}</span>} />
                    </div>
                    <Show when={section() === "walls"}>
                      <Show when={d().walls.length} fallback={<Empty title="No active walls" icon={<Megaphone size={24} />}>Open the first one from Hashwall → Walls.</Empty>}>
                        <For each={d().walls}>{(w) => <WallCard w={w} onOpen={() => navigate(`/hashwall/walls/${w.id}`)} />}</For>
                      </Show>
                    </Show>
                    <Show when={section() === "people"}>
                      <Show when={d().top_authors.length} fallback={<Empty title="Nobody posted in this window" icon={<Users size={24} />} />}>
                        <ol class="card divide-y divide-border">
                          <For each={d().top_authors}>
                            {(a, i) => (
                              <li class="flex items-center gap-3 px-3 py-2 text-[13px]">
                                <span class="tnum w-6 text-right text-xs text-muted">{i() + 1}</span>
                                <button type="button" onClick={() => navigate(`/people/${a.author}`)}>
                                  <PersonAvatar address={a.author} size={28} />
                                </button>
                                <button type="button" class="min-w-0 flex-1 text-left hover:underline" onClick={() => navigate(`/people/${a.author}`)}>
                                  <Who address={a.author} />
                                  <span class="block text-[11px] text-muted">active {shortWhen(a.last_active * 1000)}</span>
                                </button>
                                <span class="tnum text-right text-xs text-muted" title="posts · comments · reactions received">
                                  <b class="text-fg">{a.posts}</b> posts · {a.comments} comments · {a.reactions_received} ♥
                                </span>
                              </li>
                            )}
                          </For>
                        </ol>
                      </Show>
                    </Show>
                    <Show when={section() === "tags"}>
                      <Show when={d().top_hashtags.length} fallback={<Empty title="No hashtags in this window" icon={<Hash size={24} />} />}>
                        <div class="flex flex-wrap gap-2">
                          <For each={d().top_hashtags}>
                            {(h) => (
                              <button type="button" class="card flex items-center gap-2 px-3 py-2 text-[13px] hover:border-brand" onClick={() => navigate(`/explore/posts/${encodeURIComponent(h.tag)}`)}>
                                <span class="text-brand">#{h.tag}</span>
                                <span class="tnum text-xs text-muted">{h.posts} posts · {h.authors} people</span>
                              </button>
                            )}
                          </For>
                        </div>
                      </Show>
                    </Show>
                  </>
                )}
              </Show>
            </Show>
          </Show>
          <Show when={section() === "holders"}>
            <TopHolders />
          </Show>
          <Show when={section() === "providers"}>
            <Providers />
          </Show>
        </div>
      </div>
    </div>
  );
}

function ExplorePosts(props: { tag: string; onTag: (tag: string) => void }) {
  const navigate = useNavigate();
  const [draft, setDraft] = createSignal(props.tag);
  const pages = usePages(
    (before) => ipc.hashwallExplore(before, 20, props.tag || undefined),
    () => ({ tag: props.tag, tick: store.ticks().feed }),
  );
  return (
    <div>
      <div class="mb-3 flex items-center gap-2">
        <div class="relative flex-1">
          <Search size={12} class="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-muted" />
          <Input
            class="h-7 pl-6"
            placeholder="Filter by #hashtag…"
            value={draft()}
            onInput={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === "Enter") props.onTag(draft().trim().replace(/^#/, "")); }}
          />
        </div>
        <Show when={props.tag}>
          <Badge>
            #{props.tag}
            <button type="button" class="ml-1" title="Clear" onClick={() => { setDraft(""); props.onTag(""); }}>
              <X size={10} />
            </button>
          </Badge>
        </Show>
        <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => void pages.reload()}>
          <RefreshCw size={13} />
        </Button>
      </div>
      <SourceLine page={pages.source()} />
      <Show when={!pages.error()} fallback={<ErrorState error={pages.error()} onRetry={() => void pages.reload()} />}>
        <For each={pages.items()} fallback={
          <Show when={!pages.loading()}>
            <Empty title={props.tag ? `Nothing tagged #${props.tag} yet` : t("nothing_here")} icon={<Compass size={24} />}>
              {props.tag ? "Try another tag, or be the first to use it." : "Posts appear here as soon as a nearby node holds any. Nodes only sync while they are online."}
            </Empty>
          </Show>
        }>
          {(it) => <PostCard it={it} onOpen={() => navigate(`/hashwall/post/${it.id}`)} />}
        </For>
        <div ref={pages.sentinel} class="flex items-center justify-center py-3 text-xs text-muted">
          <Show when={pages.loading()}><span class="inline-block h-3 w-3 animate-spin rounded-full border border-current border-t-transparent" /></Show>
          <Show when={!pages.loading() && !pages.done() && pages.items().length}>
            <Button variant="ghost" size="sm" onClick={() => void pages.more()}>{t("explore_load_more")}</Button>
          </Show>
          <Show when={pages.done() && pages.items().length}>{t("explore_end")}</Show>
        </div>
      </Show>
    </div>
  );
}

function TopHolders() {
  const navigate = useNavigate();
  const [force, setForce] = createSignal(0);
  const [holders, { refetch }] = createResource(
    () => force(),
    async (f) => {
      if (holdersCache && f === 0) return holdersCache;
      const h = await ipc.networkHolders(100);
      holdersCache = h;
      return h;
    },
  );
  const me = () => store.status()?.address;
  const pct = (bps: number) => `${(bps / 100).toFixed(bps >= 1000 ? 1 : 2)}%`;
  return (
    <div>
      <div class="mb-3 flex items-center gap-2">
        <Crown size={14} class="text-muted" />
        <p class="flex-1 text-xs text-muted">Who holds the most HASH — read from the chain's bank module, not from any server.</p>
        <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => { holdersCache = null; setForce(force() + 1); void refetch(); }}>
          <RefreshCw size={13} />
        </Button>
      </div>
      <Show when={!holders.error} fallback={<ErrorState error={holders.error} onRetry={() => void refetch()} />}>
        <Show when={holders()} fallback={<div class="flex flex-col gap-2"><div class="skeleton h-9 w-full" /><div class="skeleton h-9 w-full" /><div class="skeleton h-9 w-full" /></div>}>
          {(h) => (
            <>
              <div class="mb-3 grid grid-cols-2 gap-2 sm:grid-cols-3">
                <Stat label="Accounts scanned" value={<span class="tnum">{h().accounts_scanned.toLocaleString()}</span>} sub={h().complete ? "complete" : "partial — chain has more accounts than one scan covers"} />
                <Stat label="HASH in these accounts" value={<span class="tnum">{formatHash(h().total_uhash, 0, 2)}</span>} />
                <Stat label="Height" value={<span class="tnum">{h().height ?? "—"}</span>} />
              </div>
              <Show when={h().holders.length} fallback={<Empty title="No balances found" />}>
                <ol class="card divide-y divide-border">
                  <For each={h().holders}>
                    {(x) => (
                      <li class={`flex items-center gap-3 px-3 py-2 text-[13px] ${x.address === me() ? "bg-surface-2" : ""}`}>
                        <span class="tnum w-8 text-right text-xs text-muted">#{x.rank}</span>
                        <button type="button" onClick={() => navigate(`/people/${x.address}`)}>
                          <PersonAvatar address={x.address} size={24} />
                        </button>
                        <span class="min-w-0 flex-1">
                          <Show when={x.username} fallback={<Mono text={x.address} head={12} tail={6} copy />}>
                            <button type="button" class="font-medium hover:underline" onClick={() => navigate(`/people/${x.address}`)}>@{x.username}</button>
                            <span class="block"><Mono text={x.address} head={12} tail={6} copy class="text-[11px] text-muted" /></span>
                          </Show>
                        </span>
                        <span class="text-right">
                          <span class="tnum block font-medium">{formatHash(x.balance_uhash, 2, 2)} HASH</span>
                          <span class="tnum block text-[11px] text-muted">{pct(x.share_bps)} of scanned</span>
                        </span>
                      </li>
                    )}
                  </For>
                </ol>
              </Show>
            </>
          )}
        </Show>
      </Show>
    </div>
  );
}

function Providers() {
  const [list, { refetch }] = createResource(() => ipc.networkProviders());
  const sorted = createMemo(() => [...(list() ?? [])].sort((a, b) => (BigInt(b.bond_uhash || "0") > BigInt(a.bond_uhash || "0") ? 1 : -1)));
  const roles = (p: ProviderStatus) => p.roles.map((r) => r.replace(/^ROLE_/, "").toLowerCase()).join(", ");
  return (
    <div>
      <div class="mb-3 flex items-center gap-2">
        <Server size={14} class="text-muted" />
        <p class="flex-1 text-xs text-muted">Registered storage and relay providers, by bond. Every node you read from is run by one of these operators.</p>
        <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => void refetch()}>
          <RefreshCw size={13} />
        </Button>
      </div>
      <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={() => void refetch()} />}>
        <Show when={list()} fallback={<div class="skeleton h-9 w-full" />}>
          <Show when={sorted().length} fallback={<Empty title="No providers registered yet" />}>
            <ol class="card divide-y divide-border">
              <For each={sorted()}>
                {(p, i) => (
                  <li class="flex items-center gap-3 px-3 py-2 text-[13px]">
                    <span class="tnum w-6 text-right text-xs text-muted">{i() + 1}</span>
                    <span class="min-w-0 flex-1">
                      <span class="flex items-center gap-2">
                        <span class="truncate font-medium">{p.moniker || "unnamed"}</span>
                        <Badge>{roles(p) || "provider"}</Badge>
                        <Show when={p.jailed}><Badge>jailed</Badge></Show>
                      </span>
                      <Mono text={p.operator} head={12} tail={6} copy class="text-[11px] text-muted" />
                    </span>
                    <span class="text-right">
                      <span class="tnum block font-medium">{formatHash(p.bond_uhash, 0, 2)} HASH</span>
                      <span class="tnum block text-[11px] text-muted">{p.declared_storage_bytes ? `${(p.declared_storage_bytes / 1e9).toFixed(0)} GB declared` : "relay only"}</span>
                    </span>
                  </li>
                )}
              </For>
            </ol>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
