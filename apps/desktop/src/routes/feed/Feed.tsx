// Hashwall: Friends / Following / Walls / Circles, all chronological.
// Walls are open topics (signed CHANNEL_CREATE events) anyone can open and,
// when open, anyone can write on; a wall's page comes from the nearest node
// one page at a time. Composer for public posts (text, media, hashtags,
// sensitive, optional wall) and Circle posts (circle picker, poll builder).
// Post view with comments and reactions pulled from the network.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Image as ImageIcon, Send, MessageSquare, Heart, Repeat2, Trash2, Plus, Users, RefreshCw, Vote, Eye, EyeOff, UserPlus, X, Pin, PinOff, Link as LinkIcon, Megaphone, Hash } from "lucide-solid";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Button, Checkbox, Dialog, Field, Input, Notice, Tabs, Textarea, Badge, Empty, Select } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Who, PersonAvatar } from "~/components/identity";
import { ipc, errText, type FeedItem, type CircleInfo, type CircleItemView, type MergedItem, type PollInput, type WallInfo, type ExplorePage } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatMs, shortWhen, splitRecipients } from "~/lib/format";
import { pickFile, confirm } from "~/lib/dialogs";
import { copyText } from "~/lib/clipboard";

type Tab = "friends" | "following" | "walls" | "circles" | "post";

const WALL_ID = /^[0-9a-f]{64}$/;

export function postText(it: FeedItem): string {
  const p = it.payload as { text?: string; comment?: string };
  return String(p.text ?? p.comment ?? "");
}
export function hashtags(it: FeedItem): string[] {
  const p = it.payload as { hashtags?: string[] };
  return Array.isArray(p.hashtags) ? p.hashtags : [];
}

/**
 * Pages of a remote timeline (Explore, a wall, a hashtag): loads one page,
 * then the next when the sentinel scrolls into view or on "Load more".
 * Dedups by id because a page boundary may re-serve one second's posts.
 */
export function usePages(fetchPage: (before: number) => Promise<ExplorePage>, key: () => unknown) {
  const [items, setItems] = createSignal<FeedItem[]>([]);
  const [next, setNext] = createSignal<number>(0);
  const [done, setDone] = createSignal(false);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [source, setSource] = createSignal<ExplorePage | null>(null);
  let inflight = false;
  const load = async (reset: boolean) => {
    if (inflight) return;
    if (!reset && done()) return;
    inflight = true;
    setLoading(true);
    setError(null);
    try {
      const page = await fetchPage(reset ? 0 : next());
      setSource(page);
      const seen = new Set(reset ? [] : items().map((i) => i.id));
      const fresh = page.items.filter((i) => !seen.has(i.id));
      setItems(reset ? fresh : [...items(), ...fresh]);
      setNext(page.next_before);
      setDone(page.next_before === 0 || (fresh.length === 0 && page.items.length === 0));
    } catch (e) {
      setError(e);
    } finally {
      setLoading(false);
      inflight = false;
    }
  };
  createEffect(() => {
    key();
    setItems([]);
    setNext(0);
    setDone(false);
    void load(true);
  });
  /** Attach to an element at the end of the list to load more when visible. */
  const sentinel = (el: HTMLElement) => {
    const obs = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting) && !done() && !loading()) void load(false);
    });
    obs.observe(el);
    onCleanup(() => obs.disconnect());
  };
  return { items, done, loading, error, source, more: () => load(false), reload: () => load(true), sentinel };
}

/** Where a page came from: node distance and operator, never an address. */
export function SourceLine(props: { page: ExplorePage | null }) {
  return (
    <Show when={props.page}>
      {(p) => (
        <Show
          when={!p().note}
          fallback={
            <p class="mb-2 rounded-md border border-border bg-surface-2 px-3 py-2 text-[11px] text-muted" data-source-peer="local" role="note">
              <span class="font-medium text-fg">From this device. </span>
              {p().note}
            </p>
          }
        >
          <p class="mb-2 text-[11px] text-muted" data-source-peer={p().source_peer}>
            {t("explore_nearest")}
            <Show when={p().source_rtt_ms !== null}> · {p().source_rtt_ms} ms</Show>
            <Show when={p().source_operator}> · operator <span class="mono">{p().source_operator.slice(0, 12)}…</span></Show>
          </p>
        </Show>
      )}
    </Show>
  );
}

function Media(props: { cid: string; mime: string; sensitive?: boolean }) {
  const [shown, setShown] = createSignal(!props.sensitive);
  const [src] = createResource(
    () => (shown() && props.mime.startsWith("image/") ? props.cid : null),
    (cid) => ipc.feedMediaFetch(cid, props.mime).then(convertFileSrc).catch(() => null),
  );
  return (
    <Show when={props.mime.startsWith("image/")} fallback={<Badge>{props.mime}</Badge>}>
      <Show when={shown()} fallback={<Button variant="secondary" size="sm" onClick={() => setShown(true)}><EyeOff size={12} /> sensitive — show</Button>}>
        <Show when={src()} fallback={<div class="skeleton h-40 w-full" />}>
          <img src={src()!} alt="" class="max-h-96 rounded-md border border-border object-contain" />
        </Show>
      </Show>
    </Show>
  );
}

export function FeedRoute() {
  const params = useParams<{ tab?: string; id?: string }>();
  const navigate = useNavigate();
  const tab = (): Tab => (params.tab as Tab) || "friends";
  const [compose, setCompose] = createSignal<false | { wall?: string }>(false);
  // Under /hashwall/walls/<id> the id is a wall; anywhere else it is a post.
  const wallId = () => (tab() === "walls" && params.id && WALL_ID.test(params.id) ? params.id : null);
  const postId = () => (params.id && !wallId() ? params.id : null);

  const [items, { refetch }] = createResource(
    () => ({ tab: tab(), tick: store.ticks().feed }),
    async (k) => {
      if (k.tab === "friends") return { kind: "public" as const, items: await ipc.feedFriends(0, 100) };
      if (k.tab === "following") return { kind: "public" as const, items: await ipc.feedFollowing(0, 100) };
      if (k.tab === "walls") return { kind: "walls" as const, items: await ipc.wallsPinned() };
      if (k.tab === "circles") return { kind: "circles" as const, items: await ipc.circlesMerged(0, 100) };
      return { kind: "public" as const, items: [] as FeedItem[] };
    },
  );

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <Show when={postId()} fallback={
        <Show when={wallId()} fallback={
          <div class="flex min-h-0 flex-1 flex-col">
            <div class="flex items-center gap-2 border-b border-border px-3">
              <Tabs
                class="flex-1 border-b-0"
                value={tab()}
                onChange={(v) => navigate(`/hashwall/${v}`)}
                tabs={[
                  { id: "friends", label: t("feed_friends") },
                  { id: "following", label: t("feed_following") },
                  { id: "walls", label: t("feed_walls") },
                  { id: "circles", label: t("feed_circles") },
                ]}
              />
              <Button variant="ghost" size="sm" title="Discover posts, walls and people across the network" onClick={() => navigate("/explore")}>
                <Hash size={13} /> {t("feed_explore")}
              </Button>
              <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => { void ipc.feedRefresh().catch(() => undefined); void refetch(); }}>
                <RefreshCw size={13} />
              </Button>
              <Button variant="brand" size="sm" onClick={() => setCompose({})}>
                <Plus size={13} /> {t("feed_post")}
              </Button>
            </div>
            <div class="min-h-0 flex-1 overflow-auto">
              <Show when={!items.error} fallback={<ErrorState error={items.error} onRetry={() => void refetch()} />}>
                <div class="mx-auto max-w-2xl px-4 py-3">
                  <Show when={tab() === "circles"}>
                    <CirclesBar />
                  </Show>
                  <Show when={items()}>
                    {(r) => (
                      <>
                        <Show when={r().kind === "public"}>
                          <Show when={(r().items as FeedItem[]).length} fallback={<Empty title={t("nothing_here")}>{tab() === "friends" ? "Posts from your contacts appear here. Find people in Explore." : "Follow people from their profile to see their posts here."}</Empty>}>
                            <For each={r().items as FeedItem[]}>{(it) => <PostCard it={it} onOpen={() => navigate(`/hashwall/${tab()}/${it.id}`)} />}</For>
                          </Show>
                        </Show>
                        <Show when={r().kind === "walls"}>
                          <WallsHome walls={r().items as WallInfo[]} onChanged={() => void refetch()} />
                        </Show>
                        <Show when={r().kind === "circles"}>
                          <Show when={(r().items as MergedItem[]).length} fallback={<Empty title="No circle posts yet">Create a Circle above, or wait for one you were added to.</Empty>}>
                            <For each={r().items as MergedItem[]}>{(m) => <CirclePost circle={m.circle} item={m.item} />}</For>
                          </Show>
                        </Show>
                      </>
                    )}
                  </Show>
                </div>
              </Show>
            </div>
          </div>
        }>
          {(id) => <WallView id={id()} onBack={() => navigate("/hashwall/walls")} onCompose={() => setCompose({ wall: id() })} />}
        </Show>
      }>
        {(id) => <PostView id={id()} onBack={() => (window.history.length > 1 ? window.history.back() : navigate(`/hashwall/${tab() === "post" ? "friends" : tab()}`))} />}
      </Show>
      <ComposeDialog open={!!compose()} onClose={() => setCompose(false)} defaultCircle={tab() === "circles"} defaultWall={(compose() || {}).wall} />
    </div>
  );
}

/** A small link to a wall by id: resolves the name from the local cache. */
export function WallChip(props: { id: string }) {
  const navigate = useNavigate();
  const [info] = createResource(
    () => props.id,
    (id) => ipc.wallsInfo(id).catch(() => null),
  );
  return (
    <button type="button" class="badge inline-flex items-center gap-1 hover:border-brand" title="Open wall" onClick={(e) => { e.stopPropagation(); navigate(`/hashwall/walls/${props.id}`); }}>
      <Megaphone size={10} /> {info()?.name ?? `${props.id.slice(0, 8)}…`}
    </button>
  );
}

export function PostCard(props: { it: FeedItem; onOpen?: () => void; full?: boolean; hideWall?: boolean }) {
  const navigate = useNavigate();
  const [thread] = createResource(
    () => (props.full ? null : props.it.id),
    // Local cache only for the counts under a card; the full view fetches.
    (id) => ipc.feedThread(id).catch(() => null),
  );
  const sensitive = () => !!(props.it.payload as { sensitive?: boolean }).sensitive;
  const isMe = () => store.status()?.address === props.it.author;
  const react = async (r: string) => {
    try {
      await ipc.feedReact(props.it.id, r);
      store.bump("feed");
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  return (
    <article class="card mb-2 p-3 text-[13px]" data-post={props.it.id}>
      <div class="flex items-center gap-2">
        <button type="button" class="shrink-0" title="Open profile" onClick={() => navigate(`/people/${props.it.author}`)}>
          <PersonAvatar address={props.it.author} size={24} />
        </button>
        <button type="button" class="min-w-0 font-medium hover:underline" title="Open profile" onClick={() => navigate(`/people/${props.it.author}`)}>
          <Who address={props.it.author} />
        </button>
        <Show when={props.it.kind === "REPOST"}>
          <Badge>
            <Repeat2 size={10} class="mr-1" /> repost
          </Badge>
        </Show>
        <Show when={props.it.kind === "REEL_CREATE"}>
          <Badge>reel</Badge>
        </Show>
        <Show when={props.it.channel && !props.hideWall}>
          <WallChip id={props.it.channel} />
        </Show>
        <span class="flex-1" />
        <span class="tnum text-[11px] text-muted" title={formatMs(props.it.timestamp * 1000)}>
          {shortWhen(props.it.timestamp * 1000)}
        </span>
        <Show when={isMe() && props.it.kind === "POST_CREATE"}>
          <button type="button" class="text-muted hover:text-fg" title="Delete post" onClick={async () => { if (await confirm("Delete this post? A tombstone is published; readers hide it.")) { await ipc.feedDelete(props.it.id).catch((e) => store.toast(errText(e), "error")); store.bump("feed"); } }}>
            <Trash2 size={12} />
          </button>
        </Show>
      </div>
      <p class={`mt-2 whitespace-pre-wrap selectable ${props.onOpen ? "cursor-pointer" : ""}`} onClick={props.onOpen}>
        {postText(props.it)}
      </p>
      <Show when={hashtags(props.it).length}>
        <p class="mt-1 flex flex-wrap gap-x-2 text-xs text-brand">
          <For each={hashtags(props.it)}>
            {(h) => (
              <button type="button" class="hover:underline" title={`Posts tagged #${h}`} onClick={(e) => { e.stopPropagation(); navigate(`/explore/posts/${encodeURIComponent(h.replace(/^#/, ""))}`); }}>
                #{h.replace(/^#/, "")}
              </button>
            )}
          </For>
        </p>
      </Show>
      <Show when={props.it.media.length}>
        <div class="mt-2 flex flex-wrap gap-2">
          <For each={props.it.media}>{([cid, mime]) => <Media cid={cid} mime={mime} sensitive={sensitive()} />}</For>
        </div>
      </Show>
      <div class="mt-2 flex items-center gap-3 text-xs text-muted">
        <button type="button" class="inline-flex items-center gap-1 hover:text-fg" onClick={() => void react("❤")} title="React">
          <Heart size={12} /> {thread()?.reactions["❤"] ?? ""}
        </button>
        <button type="button" class="inline-flex items-center gap-1 hover:text-fg" onClick={props.onOpen} title="Comments">
          <MessageSquare size={12} /> {thread()?.comments.length ?? ""}
        </button>
        <button type="button" class="inline-flex items-center gap-1 hover:text-fg" title="Repost" onClick={async () => { try { await ipc.feedRepost(props.it.id, ""); store.toast("Reposted"); store.bump("feed"); } catch (e) { store.toast(errText(e), "error"); } }}>
          <Repeat2 size={12} />
        </button>
        <Show when={props.it.visibility !== "public"}>
          <Badge>{props.it.visibility}</Badge>
        </Show>
      </div>
    </article>
  );
}

function PostView(props: { id: string; onBack: () => void }) {
  const [thread, { refetch }] = createResource(
    () => ({ id: props.id, tick: store.ticks().feed }),
    // The network's view: the post plus every comment and reaction a node
    // holds for it, so a post found in Explore opens complete.
    (k) => ipc.hashwallThread(k.id),
  );
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  return (
    <div class="mx-auto w-full max-w-2xl overflow-auto px-4 py-3">
      <button type="button" class="mb-2 text-xs text-muted hover:text-fg" onClick={props.onBack}>
        ← back
      </button>
      <Show when={!thread.error} fallback={<ErrorState error={thread.error} onRetry={() => void refetch()} />}>
        <Show when={thread()} fallback={<p class="text-xs text-muted">{thread.loading ? t("loading") : "Post not found or not fetched yet."}</p>}>
          {(th) => (
            <>
              <PostCard it={th().post} full />
              <Show when={Object.keys(th().reactions).length}>
                <p class="mb-2 text-xs text-muted">
                  <For each={Object.entries(th().reactions)}>{([r, n]) => <span class="mr-2">{r} {n}</span>}</For>
                </p>
              </Show>
              <div class="mb-3 flex items-center gap-2">
                <Input value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder="Write a comment…" onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && void submit()} />
                <Button size="sm" loading={busy()} disabled={!text().trim()} onClick={() => void submit()}>
                  <Send size={12} />
                </Button>
              </div>
              <For each={th().comments} fallback={<p class="text-xs text-muted">No comments yet.</p>}>
                {(c) => (
                  <div class="mb-1.5 rounded-md border border-border px-3 py-2 text-[13px]">
                    <div class="flex items-center gap-2 text-xs text-muted">
                      <Who address={c.author} size="sm" />
                      <span>{shortWhen(c.timestamp * 1000)}</span>
                    </div>
                    <p class="mt-0.5 whitespace-pre-wrap selectable">{postText(c)}</p>
                  </div>
                )}
              </For>
            </>
          )}
        </Show>
      </Show>
    </div>
  );
  async function submit() {
    if (!text().trim()) return;
    setBusy(true);
    try {
      await ipc.feedComment(props.id, text());
      setText("");
      store.bump("feed");
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  }
}

// ---------------------------------------------------------------------------
// Walls
// ---------------------------------------------------------------------------

export function WallCard(props: { w: WallInfo; onOpen?: () => void }) {
  return (
    <button type="button" class="card mb-2 flex w-full items-start gap-3 p-3 text-left text-[13px] hover:border-brand" onClick={props.onOpen} data-wall={props.w.id}>
      <span class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-surface-2 text-muted">
        <Megaphone size={14} />
      </span>
      <span class="min-w-0 flex-1">
        <span class="flex items-center gap-2">
          <span class="truncate font-medium">{props.w.name}</span>
          <Badge title={props.w.open_posting ? t("wall_open") : t("wall_closed")}>{props.w.open_posting ? "open" : "announcements"}</Badge>
          <Show when={props.w.pinned}>
            <Pin size={11} class="text-muted" />
          </Show>
        </span>
        <Show when={props.w.description}>
          <span class="mt-0.5 block truncate text-xs text-muted">{props.w.description}</span>
        </Show>
        <span class="mt-1 flex flex-wrap items-center gap-x-3 text-[11px] text-muted">
          <span class="inline-flex items-center gap-1">by <Who address={props.w.creator} size="sm" /></span>
          <Show when={props.w.posts}><span class="tnum">{props.w.posts} posts · {props.w.authors} people</span></Show>
          <Show when={props.w.last_post}><span>active {shortWhen(props.w.last_post * 1000)}</span></Show>
        </span>
      </span>
    </button>
  );
}

function WallsHome(props: { walls: WallInfo[]; onChanged: () => void }) {
  const navigate = useNavigate();
  const [create, setCreate] = createSignal(false);
  const [open, setOpen] = createSignal("");
  return (
    <div>
      <div class="mb-3 flex flex-wrap items-center gap-2">
        <Button variant="brand" size="sm" onClick={() => setCreate(true)}>
          <Plus size={12} /> {t("wall_new")}
        </Button>
        <Button variant="secondary" size="sm" onClick={() => navigate("/explore/walls")}>
          <Hash size={12} /> Find walls
        </Button>
        <span class="flex-1" />
        <Input class="h-7 w-64" placeholder="Open a wall by id or link…" value={open()} onInput={(e) => setOpen(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") { const m = open().trim().toLowerCase().match(/[0-9a-f]{64}/); if (m) navigate(`/hashwall/walls/${m[0]}`); else store.toast("That is not a wall id or link", "error"); } }} />
      </div>
      <Show when={props.walls.length} fallback={
        <Empty title="No walls pinned yet" icon={<Megaphone size={24} />}>
          A wall is a topic — a protest, a profession, a town, a project. Open one and everyone on the network can write on it, or find active walls in Explore.
        </Empty>
      }>
        <For each={props.walls}>{(w) => <WallCard w={w} onOpen={() => navigate(`/hashwall/walls/${w.id}`)} />}</For>
      </Show>
      <CreateWallDialog open={create()} onClose={() => { setCreate(false); props.onChanged(); }} onCreated={(w) => navigate(`/hashwall/walls/${w.id}`)} />
    </div>
  );
}

export function CreateWallDialog(props: { open: boolean; onClose: () => void; onCreated?: (w: WallInfo) => void }) {
  const [name, setName] = createSignal("");
  const [desc, setDesc] = createSignal("");
  const [openPosting, setOpenPosting] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const registered = () => store.identity()?.this_device_registered !== false;
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("wall_new")} description="A public topic on the network. Its name and description are signed by you and visible to everyone." width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="Name" hint="2–64 characters: letters, digits, spaces, _ - .">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={64} placeholder="ArchevnebiSaqartveloshi" />
        </Field>
        <Field label="Description">
          <Textarea value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} rows={3} maxLength={2000} placeholder="What is this wall for, and who should post here?" />
        </Field>
        <Checkbox checked={openPosting()} onChange={setOpenPosting} label={t("wall_open")} hint="Off: an announcement wall where only you post; everyone can still read and comment." />
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <Show when={!registered()}>
          <Notice strong>Register this PC (Wallet → Devices) before opening a wall.</Notice>
        </Show>
        <div class="flex justify-end">
          <Button
            loading={busy()}
            disabled={!registered() || name().trim().length < 2}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const w = await ipc.wallsCreate(name(), desc(), openPosting());
                store.toast(`Wall "${w.name}" opened`);
                store.bump("feed");
                setName(""); setDesc(""); setOpenPosting(true);
                props.onClose();
                props.onCreated?.(w);
              } catch (e) {
                setError(errText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            Open wall
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function WallView(props: { id: string; onBack: () => void; onCompose: () => void }) {
  const navigate = useNavigate();
  const [info, { refetch: refetchInfo }] = createResource(
    () => props.id,
    (id) => ipc.wallsInfo(id),
  );
  const pages = usePages((before) => ipc.wallsPage(props.id, before, 20), () => ({ id: props.id, tick: store.ticks().feed }));
  const me = () => store.status()?.address;
  const canPost = () => !!info() && (info()!.open_posting || info()!.creator === me());
  const link = () => `hashgram://wall/${props.id}`;
  const [invite, setInvite] = createSignal("");
  const togglePin = async () => {
    const w = info();
    if (!w) return;
    try {
      await ipc.wallsPin(w.id, !w.pinned);
      store.toast(w.pinned ? "Unpinned" : "Pinned to Walls");
      void refetchInfo();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  const sendInvite = () => {
    const w = info();
    if (!w) return;
    const to = invite().trim();
    const subject = `Join the wall "${w.name}" on Hashgram`;
    const body = `I opened a wall on Hashgram: "${w.name}".\n${w.description ? w.description + "\n" : ""}\nOpen it in Hashgram One: ${link()}\n(Or paste the id into Hashwall → Walls: ${w.id})`;
    navigate(`/mail/inbox?compose=1${to ? `&q=${encodeURIComponent(to)}` : ""}&subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`);
  };
  return (
    <div class="mx-auto w-full max-w-2xl overflow-auto px-4 py-3" data-wall-view={props.id}>
      <button type="button" class="mb-2 text-xs text-muted hover:text-fg" onClick={props.onBack}>
        ← {t("feed_walls")}
      </button>
      <Show when={!info.error} fallback={<ErrorState error={info.error} onRetry={() => void refetchInfo()} />}>
        <Show when={info()} fallback={<div class="skeleton h-20 w-full" />}>
          {(w) => (
            <header class="card mb-3 p-4">
              <div class="flex items-start gap-3">
                <span class="flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-surface-2 text-muted">
                  <Megaphone size={18} />
                </span>
                <div class="min-w-0 flex-1">
                  <h1 class="flex flex-wrap items-center gap-2 text-base font-semibold">
                    {w().name}
                    <Badge title={w().open_posting ? t("wall_open") : t("wall_closed")}>{w().open_posting ? "open — anyone can post" : "announcements"}</Badge>
                  </h1>
                  <Show when={w().description}>
                    <p class="mt-1 whitespace-pre-wrap text-[13px] text-muted selectable">{w().description}</p>
                  </Show>
                  <p class="mt-1 flex flex-wrap items-center gap-x-3 text-[11px] text-muted">
                    <span class="inline-flex items-center gap-1">opened by <Who address={w().creator} size="sm" /></span>
                    <span>{formatMs(w().created_at * 1000)}</span>
                    <span class="mono selectable" title="wall id">{w().id.slice(0, 16)}…</span>
                  </p>
                </div>
              </div>
              <div class="mt-3 flex flex-wrap items-center gap-2">
                <Button variant="brand" size="sm" disabled={!canPost()} title={canPost() ? "" : "Only the creator posts on this wall"} onClick={props.onCompose}>
                  <Plus size={12} /> {t("feed_post")}
                </Button>
                <Button variant="secondary" size="sm" onClick={() => void togglePin()}>
                  <Show when={w().pinned} fallback={<><Pin size={12} /> {t("wall_pin")}</>}><PinOff size={12} /> {t("wall_unpin")}</Show>
                </Button>
                <Button variant="secondary" size="sm" onClick={() => void copyText(link())} title={link()}>
                  <LinkIcon size={12} /> {t("wall_share_link")}
                </Button>
                <span class="flex-1" />
                <Input class="h-7 w-44" placeholder="@name or address" value={invite()} onInput={(e) => setInvite(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && sendInvite()} />
                <Button variant="secondary" size="sm" onClick={sendInvite} title="Send an invitation by Hashgram mail">
                  <UserPlus size={12} /> {t("wall_invite")}
                </Button>
              </div>
            </header>
          )}
        </Show>
      </Show>
      <SourceLine page={pages.source()} />
      <Show when={!pages.error()} fallback={<ErrorState error={pages.error()} onRetry={() => void pages.reload()} />}>
        <For each={pages.items()} fallback={<Show when={!pages.loading()}><Empty title="Nothing on this wall yet">{canPost() ? "Be the first to write here." : "The creator has not posted yet."}</Empty></Show>}>
          {(it) => <PostCard it={it} hideWall onOpen={() => navigate(`/hashwall/post/${it.id}`)} />}
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

// ---------------------------------------------------------------------------
// Circles
// ---------------------------------------------------------------------------

function CirclesBar() {
  const [circles, { refetch }] = createResource(
    () => store.ticks().circles,
    () => ipc.circlesList().catch(() => [] as CircleInfo[]),
  );
  const [create, setCreate] = createSignal(false);
  const [manage, setManage] = createSignal<CircleInfo | null>(null);
  return (
    <div class="mb-3 flex flex-wrap items-center gap-1.5">
      <For each={circles() ?? []}>
        {(c) => (
          <button type="button" class="badge-strong h-6 px-2.5 hover:border-brand" title={`${c.members.length} members${c.owner ? " · you created it" : ""}`} onClick={() => setManage(c)}>
            <Users size={10} class="mr-1" /> {c.name} · {c.members.length}
          </button>
        )}
      </For>
      <Button variant="ghost" size="sm" onClick={() => setCreate(true)}>
        <Plus size={12} /> New Circle
      </Button>
      <CreateCircleDialog open={create()} onClose={() => { setCreate(false); void refetch(); }} />
      <ManageCircleDialog circle={manage()} onClose={() => { setManage(null); void refetch(); }} />
    </div>
  );
}

function CreateCircleDialog(props: { open: boolean; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [desc, setDesc] = createSignal("");
  const [members, setMembers] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  return (
    <Dialog open={props.open} onClose={props.onClose} title="New Circle" description="A private group. Posts travel encrypted to the members' devices and nowhere else." width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="Name">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={128} />
        </Field>
        <Field label="Description">
          <Input value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} maxLength={512} />
        </Field>
        <Field label="Members" hint="@names or addresses, comma-separated. Each must have a device on chain.">
          <Input value={members()} onInput={(e) => setMembers(e.currentTarget.value)} placeholder="@alice, @bob" />
        </Field>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <div class="flex justify-end">
          <Button
            loading={busy()}
            disabled={!name().trim() || !members().trim()}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await ipc.circlesCreate(name(), desc(), splitRecipients(members()));
                store.toast("Circle created");
                store.bump("circles", "feed");
                setName(""); setDesc(""); setMembers("");
                props.onClose();
              } catch (e) {
                setError(errText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            Create
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function ManageCircleDialog(props: { circle: CircleInfo | null; onClose: () => void }) {
  const [add, setAdd] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const me = () => store.status()?.address;
  const act = async (f: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await f();
      if (done) store.toast(done);
      store.bump("circles", "feed");
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={!!props.circle} onClose={props.onClose} title={props.circle?.name ?? ""} description={props.circle?.description} width="max-w-md">
      <Show when={props.circle}>
        {(c) => (
          <div class="flex flex-col gap-3">
            <ul class="card divide-y divide-border">
              <For each={c().members}>
                {(m) => (
                  <li class="flex items-center gap-2 px-3 py-1.5 text-[13px]">
                    <span class="flex-1"><Who address={m} /></span>
                    <Show when={m !== me()}>
                      <Button variant="ghost" size="sm" disabled={busy()} onClick={() => act(() => ipc.circlesRemoveMember(c().id, m), "Removed")}>
                        <X size={12} />
                      </Button>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
            <div class="flex items-center gap-2">
              <Input value={add()} onInput={(e) => setAdd(e.currentTarget.value)} placeholder="@name or address" />
              <Button size="sm" variant="secondary" disabled={!add().trim() || busy()} onClick={() => act(() => ipc.circlesAddMember(c().id, add()).then(() => setAdd("")), "Added")}>
                <UserPlus size={12} /> Add
              </Button>
            </div>
            <p class="text-[11px] text-muted">Adding a member starts a new encryption epoch: they do not see earlier posts. Any member may add members; Circles have no roles.</p>
            <div class="flex justify-end">
              <Button variant="danger" size="sm" disabled={busy()} onClick={async () => { if (await confirm(`Leave "${c().name}"?`)) { await act(() => ipc.circlesLeave(c().id), "Left the circle"); props.onClose(); } }}>
                Leave circle
              </Button>
            </div>
          </div>
        )}
      </Show>
    </Dialog>
  );
}

export function CirclePost(props: { circle: string; item: CircleItemView }) {
  const [comments, { refetch }] = createResource(
    () => ({ c: props.circle, p: props.item.id, tick: store.ticks().circles }),
    (k) => ipc.circlesComments(k.c, k.p).catch(() => [] as CircleItemView[]),
  );
  const [text, setText] = createSignal("");
  const [showComments, setShowComments] = createSignal(false);
  const [voted, setVoted] = createSignal<number[]>([]);
  const isMe = () => store.status()?.address === props.item.author;
  const totalVotes = createMemo(() => Object.values(props.item.votes).reduce((a, b) => a + b, 0));
  const act = async (f: () => Promise<unknown>) => {
    try {
      await f();
      store.bump("circles", "feed");
      void refetch();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  return (
    <article class={`card mb-2 p-3 text-[13px] ${props.item.deleted ? "opacity-50" : ""}`} data-circle-post={props.item.id}>
      <div class="flex items-center gap-2">
        <PersonAvatar address={props.item.author} size={24} />
        <span class="font-medium"><Who address={props.item.author} /></span>
        <Badge brand title="private circle post">circle</Badge>
        <span class="flex-1" />
        <span class="tnum text-[11px] text-muted">{shortWhen(props.item.at_ms)}</span>
        <Show when={isMe() && !props.item.deleted}>
          <button type="button" class="text-muted hover:text-fg" title="Delete" onClick={() => void act(() => ipc.circlesDelete(props.circle, props.item.id))}>
            <Trash2 size={12} />
          </button>
        </Show>
      </div>
      <p class="mt-2 whitespace-pre-wrap selectable">{props.item.deleted ? "(deleted)" : props.item.text}</p>
      <Show when={props.item.media.length}>
        <div class="mt-2 flex flex-wrap gap-2">
          <For each={props.item.media}>{(m, i) => <CircleMedia circle={props.circle} item={props.item.id} index={i()} mime={m.mime} />}</For>
        </div>
      </Show>
      <Show when={props.item.poll}>
        {(poll) => (
          <div class="mt-2 rounded-md border border-border p-2" data-poll>
            <p class="mb-1 flex items-center gap-1 font-medium">
              <Vote size={12} /> {poll().question}
            </p>
            <For each={poll().options}>
              {(o, i) => {
                const n = () => props.item.votes[String(i())] ?? 0;
                const pct = () => (totalVotes() ? Math.round((n() / totalVotes()) * 100) : 0);
                return (
                  <button
                    type="button"
                    class="relative mb-1 flex w-full items-center justify-between overflow-hidden rounded border border-border px-2 py-1 text-left text-xs hover:border-brand"
                    onClick={() => {
                      const next = poll().multiple_choice ? (voted().includes(i()) ? voted().filter((x) => x !== i()) : [...voted(), i()]) : [i()];
                      setVoted(next);
                      void act(() => ipc.circlesVote(props.circle, props.item.id, next));
                    }}
                    data-option={i()}
                  >
                    <span class="absolute inset-y-0 left-0 bg-surface-2" style={{ width: `${pct()}%` }} aria-hidden="true" />
                    <span class="relative">{o}</span>
                    <span class="tnum relative text-muted" data-votes={n()}>
                      {n()} · {pct()}%
                    </span>
                  </button>
                );
              }}
            </For>
            <p class="text-[11px] text-muted">
              {totalVotes()} vote{totalVotes() === 1 ? "" : "s"}
              {poll().multiple_choice ? " · multiple choice" : ""}
              {poll().closes_at_ms ? ` · closes ${formatMs(poll().closes_at_ms)}` : ""}
            </p>
          </div>
        )}
      </Show>
      <div class="mt-2 flex items-center gap-3 text-xs text-muted">
        <button type="button" class="inline-flex items-center gap-1 hover:text-fg" onClick={() => void act(() => ipc.circlesReact(props.circle, props.item.id, "❤"))}>
          <Heart size={12} /> {props.item.reactions["❤"] ?? ""}
        </button>
        <button type="button" class="inline-flex items-center gap-1 hover:text-fg" onClick={() => setShowComments((v) => !v)}>
          <MessageSquare size={12} /> {comments()?.length ?? ""}
        </button>
      </div>
      <Show when={showComments()}>
        <div class="mt-2 border-t border-border pt-2">
          <For each={comments() ?? []}>
            {(c) => (
              <div class="mb-1 text-xs">
                <span class="mr-1 font-medium"><Who address={c.author} size="sm" /></span>
                <span class="selectable">{c.deleted ? "(deleted)" : c.text}</span>
              </div>
            )}
          </For>
          <div class="mt-1 flex items-center gap-2">
            <Input class="h-7" value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder="Comment…" onKeyDown={(e) => e.key === "Enter" && text().trim() && void act(() => ipc.circlesComment(props.circle, props.item.id, text()).then(() => setText("")))} />
          </div>
        </div>
      </Show>
    </article>
  );
}

function CircleMedia(props: { circle: string; item: string; index: number; mime: string }) {
  const [src] = createResource(
    () => (props.mime.startsWith("image/") ? `${props.item}/${props.index}` : null),
    () => ipc.circlesMediaFetch(props.circle, props.item, props.index).then(convertFileSrc).catch(() => null),
  );
  return (
    <Show when={props.mime.startsWith("image/")} fallback={<Badge>{props.mime}</Badge>}>
      <Show when={src()} fallback={<div class="skeleton h-32 w-48" />}>
        <img src={src()!} alt="" class="max-h-80 rounded-md border border-border object-contain" />
      </Show>
    </Show>
  );
}

function ComposeDialog(props: { open: boolean; onClose: () => void; defaultCircle?: boolean; defaultWall?: string }) {
  const [text, setText] = createSignal("");
  const [tags, setTags] = createSignal("");
  const [media, setMedia] = createSignal<string[]>([]);
  const [sensitive, setSensitive] = createSignal(false);
  // "public" | "wall:<id>" | <circle id>
  const [target, setTarget] = createSignal<string>("public");
  createEffect(() => {
    if (props.open) setTarget(props.defaultWall ? `wall:${props.defaultWall}` : "public");
  });
  const isPublic = () => target() === "public" || target().startsWith("wall:");
  const wallOf = () => (target().startsWith("wall:") ? target().slice(5) : undefined);
  const [walls] = createResource(
    () => (props.open ? store.ticks().feed : null),
    () => ipc.wallsPinned().catch(() => [] as WallInfo[]),
  );
  // The wall we were opened from may not be pinned; still offer it.
  const wallOptions = createMemo(() => {
    const list = [...(walls() ?? [])];
    const d = props.defaultWall;
    if (d && !list.some((w) => w.id === d)) list.unshift({ id: d, name: `${d.slice(0, 8)}…`, description: "", creator: "", open_posting: true, created_at: 0, posts: 0, authors: 0, last_post: 0, pinned: false });
    const me = store.status()?.address;
    return list.filter((w) => w.open_posting || !w.creator || w.creator === me).map((w) => ({ value: `wall:${w.id}`, label: `Wall: ${w.name}` }));
  });
  const [poll, setPoll] = createSignal(false);
  const [question, setQuestion] = createSignal("");
  const [options, setOptions] = createSignal<string[]>(["", ""]);
  const [multi, setMulti] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const registered = () => store.identity()?.this_device_registered !== false;
  const [circles] = createResource(
    () => (props.open ? store.ticks().circles : null),
    () => ipc.circlesList().catch(() => [] as CircleInfo[]),
  );
  const submit = async () => {
    if (!registered()) {
      setError("Finish identity setup before posting: receive at least 0.01 HASH, then register this PC in Wallet → Devices.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (isPublic()) {
        await ipc.feedPost(text(), tags().split(/[\s,#]+/).filter(Boolean), media(), sensitive(), wallOf());
        store.bump("feed");
      } else {
        const p: PollInput | undefined = poll() ? { question: question(), options: options().filter((o) => o.trim()), multiple_choice: multi(), closes_at_ms: 0 } : undefined;
        await ipc.circlesPost(target(), text(), media(), p);
        store.bump("circles", "feed");
      }
      store.toast("Posted");
      setText(""); setTags(""); setMedia([]); setSensitive(false); setPoll(false); setQuestion(""); setOptions(["", ""]);
      props.onClose();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("feed_post")} width="max-w-lg">
      <div class="flex flex-col gap-3">
        <Select
          value={target()}
          onChange={setTarget}
          aria-label="audience"
          options={[
            { value: "public", label: "Public — everyone on the network" },
            ...wallOptions(),
            ...((circles() ?? []).map((c) => ({ value: c.id, label: `Circle: ${c.name} (${c.members.length})` }))),
          ]}
        />
        <Show when={props.defaultCircle && !(circles() ?? []).length}>
          <Notice>You have no Circles yet. Create one from the Circles tab to post privately.</Notice>
        </Show>
        <Textarea value={text()} onInput={(e) => setText(e.currentTarget.value)} rows={5} placeholder={target() === "public" ? "Say something to everyone…" : wallOf() ? "Write on the wall…" : "Say something to the circle…"} maxLength={64 * 1024} />
        <Show when={isPublic()}>
          <Field label="Hashtags">
            <Input value={tags()} onInput={(e) => setTags(e.currentTarget.value)} placeholder="#hashgram #mainnet" />
          </Field>
          <Checkbox checked={sensitive()} onChange={setSensitive} label="Sensitive media" hint="Readers click before media shows." />
        </Show>
        <Show when={!isPublic()}>
          <Checkbox checked={poll()} onChange={setPoll} label="Add a poll" />
          <Show when={poll()}>
            <div class="card flex flex-col gap-2 p-3" data-testid="poll-builder">
              <Input value={question()} onInput={(e) => setQuestion(e.currentTarget.value)} placeholder="Question" />
              <For each={options()}>
                {(o, i) => (
                  <div class="flex items-center gap-1">
                    <Input value={o} onInput={(e) => setOptions(options().map((x, j) => (j === i() ? e.currentTarget.value : x)))} placeholder={`Option ${i() + 1}`} />
                    <Show when={options().length > 2}>
                      <Button variant="ghost" size="icon-sm" onClick={() => setOptions(options().filter((_, j) => j !== i()))}>
                        <X size={12} />
                      </Button>
                    </Show>
                  </div>
                )}
              </For>
              <div class="flex items-center gap-2">
                <Button variant="ghost" size="sm" disabled={options().length >= 12} onClick={() => setOptions([...options(), ""])}>
                  <Plus size={12} /> option
                </Button>
                <Checkbox checked={multi()} onChange={setMulti} label="Multiple choice" />
              </div>
            </div>
          </Show>
        </Show>
        <div class="flex items-center gap-2">
          <Button variant="secondary" size="sm" disabled={media().length >= 20} onClick={async () => { const f = await pickFile({ multiple: true, filters: [{ name: "Media", extensions: ["png", "jpg", "jpeg", "gif", "webp", "mp4", "webm", "mp3", "m4a"] }] }); setMedia([...media(), ...f].slice(0, 20)); }}>
            <ImageIcon size={12} /> Media ({media().length}/20)
          </Button>
          <span class="truncate text-xs text-muted">{media().map((m) => m.split(/[\\/]/).pop()).join(", ")}</span>
        </div>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <Show when={!registered()}>
          <Notice strong title="Finish identity setup">
            Nodes accept signed posts only from an active device. Receive at least 0.01 HASH, then register this PC once in Wallet → Devices.
            <Button class="mt-2" size="sm" onClick={() => { props.onClose(); window.location.hash = "/wallet/devices"; }}>
              Open Wallet → Devices
            </Button>
          </Notice>
        </Show>
        <div class="flex items-center justify-between">
          <span class="text-[11px] text-muted">
            <Show when={isPublic()} fallback={<><Eye size={10} class="mr-1 inline" />Encrypted to the circle's members only.</>}>
              {wallOf() ? "Public on this wall and signed by your device key. " : "Public and signed by your device key. "}Anyone can read it; you can delete it later (a tombstone).
            </Show>
          </span>
          <Button loading={busy()} disabled={!registered() || (!text().trim() && !media().length && !(poll() && question().trim()))} onClick={submit}>
            <Send size={12} /> {t("feed_post")}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
