// Feed: chronological, following-only, every event signature-verified and
// its device resolved on chain before display. Stories ring on top.
// Composer with media. Reels, channels and profiles live on their own
// routes and share the PostCard.
import { createResource, createSignal, For, Show, onMount, onCleanup, createMemo } from "solid-js";
import { A, useNavigate, useSearchParams } from "@solidjs/router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Heart, MessageCircle, Repeat2, Image as ImageIcon, RefreshCw, ShieldAlert, Film, Plus, Trash2 } from "lucide-solid";
import { Button, Card, Notice, Skeleton, Empty, Badge, Dialog, Field, Input, Textarea } from "~/components/ui";
import { PersonLabel, Avatar } from "~/components/identity";
import { VirtualList } from "~/components/VirtualList";
import { ipc, on, pick, str, arr, type EventView, type MediaView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { person } from "~/lib/people";
import { relTime } from "~/lib/format";
import { mediaUrl } from "~/lib/media";

export function Feed() {
  const [params] = useSearchParams();
  const tag = () => (params.tag ? String(params.tag) : undefined);
  const [page, { refetch }] = createResource(
    () => tag() ?? "",
    (t) => ipc.feed(undefined, t || undefined, undefined, 200).catch((e) => ({ events: [], hidden: 0, authors: [], error: String(e) })),
  );
  const [stories] = createResource(() => ipc.feed(["STORY_CREATE"], undefined, undefined, 50).catch(() => null));
  const [busy, setBusy] = createSignal(false);
  const [composer, setComposer] = createSignal(false);

  onMount(async () => {
    const un = await on("feed:changed", () => void refetch());
    onCleanup(() => un());
  });
  const refresh = async () => {
    setBusy(true);
    try {
      const n = await ipc.feedRefresh();
      store.toast(n ? `${n} new event(s)` : "Feed is up to date");
      await refetch();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const liveStories = createMemo(() => {
    const now = Math.floor(Date.now() / 1000);
    return (stories()?.events ?? []).filter((e) => Number(str(pick(e.payload, "expires_at"), "0")) > now);
  });

  return (
    <div class="flex h-full flex-col">
      <div class="flex items-center gap-3 border-b border-border px-6 py-3">
        <h1 class="page-title">{tag() ? `#${tag()}` : "Feed"}</h1>
        <Show when={tag()}>
          <A href="/feed" class="text-xs text-muted hover:text-fg">clear</A>
        </Show>
        <span class="flex-1" />
        <Show when={(page() as { hidden?: number } | undefined)?.hidden}>
          <Badge title="Events whose signature or device did not verify against the chain were hidden.">
            <ShieldAlert size={10} /> {(page() as { hidden: number }).hidden} hidden
          </Badge>
        </Show>
        <Button size="sm" variant="secondary" onClick={refresh} loading={busy()}>
          <RefreshCw size={12} /> Refresh
        </Button>
        <Button size="sm" onClick={() => setComposer(true)}>
          <Plus size={12} /> Post
        </Button>
      </div>
      <Show when={liveStories().length}>
        <div class="flex gap-3 overflow-x-auto border-b border-border px-6 py-3">
          <For each={liveStories()}>
            {(s) => <StoryRing ev={s} />}
          </For>
        </div>
      </Show>
      <div class="min-h-0 flex-1">
        <Show when={!page.loading} fallback={<div class="p-6"><Skeleton lines={6} /></div>}>
          <Show when={page()?.events.length} fallback={<Empty title={tag() ? "Nothing with this hashtag yet" : "Your feed is empty"}>{(page() as { error?: string })?.error ?? "Follow people by address or @username (search, or a profile) and their posts appear here in order. No ranking, no suggestions: there is no server to compute them."}</Empty>}>
            <VirtualList items={page()!.events} estimateSize={160} key={(e) => e.id}>
              {(e) => (
                <div class="mx-auto max-w-2xl px-6 py-2">
                  <PostCard ev={e} onChanged={() => void refetch()} />
                </div>
              )}
            </VirtualList>
          </Show>
        </Show>
      </div>
      <Composer open={composer()} onClose={() => setComposer(false)} onPosted={() => void refetch()} />
    </div>
  );
}

function StoryRing(props: { ev: EventView }) {
  const [open, setOpen] = createSignal(false);
  const p = () => person(props.ev.author);
  return (
    <>
      <button type="button" class="flex w-16 shrink-0 flex-col items-center gap-1" onClick={() => setOpen(true)}>
        <span class="rounded-full border-2 border-fg p-0.5">
          <Avatar address={props.ev.author} size={44} />
        </span>
        <span class="mono w-full truncate text-[10px] text-muted">{p().username ? `@${p().username}` : props.ev.author.slice(0, 10)}</span>
      </button>
      <Dialog open={open()} onClose={() => setOpen(false)} title="Story" width="max-w-md">
        <PersonLabel person={p()} />
        <Show when={props.ev.media[0]}>
          <div class="mt-3">
            <Media m={props.ev.media[0]!} />
          </div>
        </Show>
        <p class="mt-2 text-sm">{str(pick(props.ev.payload, "caption"))}</p>
        <p class="mt-1 text-xs text-muted">Expires {relTime(Number(str(pick(props.ev.payload, "expires_at"), "0"))).replace(" ago", " from now")}</p>
      </Dialog>
    </>
  );
}

export function Media(props: { m: MediaView; class?: string }) {
  const [url] = createResource(() => props.m, (m) => mediaUrl(m).catch((e) => { store.toast(String(e), "error"); return ""; }));
  return (
    <Show when={url()} fallback={<div class={`skeleton h-64 w-full ${props.class ?? ""}`} />}>
      <Show when={props.m.kind === "video"} fallback={<img src={url()} alt="" class={`max-h-[70vh] w-full rounded-md object-contain ${props.class ?? ""}`} />}>
        <video src={url()} controls playsinline autoplay={store.settings()?.media.autoplay ?? true} muted loop class={`max-h-[70vh] w-full rounded-md ${props.class ?? ""}`} />
      </Show>
    </Show>
  );
}

export function PostCard(props: { ev: EventView; onChanged: () => void; compact?: boolean }) {
  const navigate = useNavigate();
  const e = () => props.ev;
  const p = () => person(e().author);
  const [comment, setComment] = createSignal("");
  const [showComments, setShowComments] = createSignal(false);
  const [thread, { refetch: refetchThread }] = createResource(() => (showComments() ? e().id : null), (id) => ipc.postThread(id).catch(() => [null, []] as [EventView | null, EventView[]]));
  const me = () => store.status()?.address ?? "";
  const text = () => str(pick(e().payload, "text")) || str(pick(e().payload, "caption")) || str(pick(e().payload, "comment"));
  const tags = () => arr(pick(e().payload, "hashtags")).map((t) => str(t));
  const likes = () => Object.values(e().reactions).reduce((a, b) => a + b, 0);
  const react = async () => {
    try {
      await ipc.socialReact(e().id, e().my_reaction ? "" : "❤️");
      props.onChanged();
    } catch (err) {
      store.toast(String(err), "error");
    }
  };
  const doRepost = async () => {
    try {
      await ipc.repost(e().id);
      store.toast("Reposted");
      props.onChanged();
    } catch (err) {
      store.toast(String(err), "error");
    }
  };
  const sendComment = async () => {
    if (!comment().trim()) return;
    try {
      await ipc.commentCreate(e().id, comment());
      setComment("");
      await refetchThread();
      props.onChanged();
    } catch (err) {
      store.toast(String(err), "error");
    }
  };
  return (
    <Card class="p-4">
      <div class="flex items-start gap-3">
        <button type="button" onClick={() => navigate(`/profile/${e().author}`)}>
          <Avatar address={e().author} size={36} />
        </button>
        <div class="min-w-0 flex-1">
          <div class="flex items-baseline gap-2">
            <button type="button" class="min-w-0 text-left" onClick={() => navigate(`/profile/${e().author}`)}>
              <PersonLabel person={p()} />
            </button>
            <span class="mono text-[10px] text-muted">{relTime(e().timestamp)}</span>
            <Show when={e().kind !== "POST_CREATE"}>
              <Badge>{e().kind === "REEL_CREATE" ? <><Film size={10} /> reel</> : e().kind.toLowerCase().replace("_create", "")}</Badge>
            </Show>
            <Show when={e().author === me()}>
              <Badge>you</Badge>
            </Show>
          </div>
          <Show when={text()}>
            <p class="selectable mt-1 whitespace-pre-wrap break-words text-sm">{text()}</p>
          </Show>
          <Show when={tags().length}>
            <p class="mt-1 flex flex-wrap gap-2 text-xs">
              <For each={tags()}>{(t) => <A href={`/feed?tag=${encodeURIComponent(t)}`} class="text-muted hover:text-fg">#{t}</A>}</For>
            </p>
          </Show>
          <Show when={e().media.length && !props.compact}>
            <div class="mt-2 grid gap-2">
              <For each={e().media}>{(m) => <Media m={m} />}</For>
            </div>
          </Show>
          <div class="mt-2 flex items-center gap-4 text-xs text-muted">
            <button type="button" class={`flex items-center gap-1 hover:text-fg ${e().my_reaction ? "text-fg" : ""}`} onClick={react}>
              <Heart size={14} fill={e().my_reaction ? "#ffffff" : "none"} /> {likes()}
            </button>
            <button type="button" class="flex items-center gap-1 hover:text-fg" onClick={() => setShowComments((v) => !v)}>
              <MessageCircle size={14} /> {e().comments}
            </button>
            <button type="button" class="flex items-center gap-1 hover:text-fg" onClick={doRepost}>
              <Repeat2 size={14} /> {e().reposts}
            </button>
            <span class="flex-1" />
            <span class="mono text-[10px]" title={`signed by device ${e().device}`}>verified · #{e().sequence}</span>
          </div>
          <Show when={showComments()}>
            <div class="mt-3 border-t border-border pt-3">
              <Show when={thread()} fallback={<Skeleton lines={2} />}>
                <For each={thread()![1]}>
                  {(c) => (
                    <div class="mb-2 flex items-start gap-2 text-sm">
                      <Avatar address={c.author} size={24} />
                      <div class="min-w-0">
                        <PersonLabel person={person(c.author)} size="sm" />
                        <p class="selectable whitespace-pre-wrap">{str(pick(c.payload, "text"))}</p>
                      </div>
                    </div>
                  )}
                </For>
              </Show>
              <div class="flex gap-2">
                <Input value={comment()} onInput={(ev) => setComment(ev.currentTarget.value)} placeholder="Write a comment" onKeyDown={(ev) => ev.key === "Enter" && void sendComment()} />
                <Button size="sm" class="h-9" onClick={sendComment} disabled={!comment().trim()}>Send</Button>
              </div>
            </div>
          </Show>
        </div>
      </div>
    </Card>
  );
}

export function Composer(props: { open: boolean; onClose: () => void; onPosted: () => void; channel?: string; reel?: boolean; story?: boolean }) {
  const [text, setText] = createSignal("");
  const [tags, setTags] = createSignal("");
  const [media, setMedia] = createSignal<MediaView[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const tagList = () => tags().split(/[\s,#]+/).map((t) => t.trim()).filter(Boolean);
  const addMedia = async () => {
    const path = await openDialog({ multiple: false, title: props.reel ? "Choose an MP4 (≤ 100 MB)" : "Choose an image or video" });
    if (!path || typeof path !== "string") return;
    setBusy(true);
    setErr(null);
    try {
      const m = await ipc.mediaUpload(path);
      setMedia((l) => [...l, m]);
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  const publish = async () => {
    setBusy(true);
    setErr(null);
    try {
      if (props.reel) {
        const v = media().find((m) => m.kind === "video");
        if (!v) throw new Error("choose a video first");
        await ipc.reelPublish(v, text(), tagList());
      } else if (props.story) {
        const m = media()[0];
        if (!m) throw new Error("choose an image or video first");
        await ipc.storyPublish(m, text());
      } else {
        await ipc.postCreate(text(), tagList(), props.channel, undefined, media());
      }
      setText("");
      setTags("");
      setMedia([]);
      props.onPosted();
      props.onClose();
      store.toast("Published and signed by this device");
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open={props.open}
      onClose={props.onClose}
      title={props.reel ? "New reel" : props.story ? "New story (24 h)" : props.channel ? "Post to channel" : "New post"}
      description="Public. Signed by this device's key; anyone can verify it against the chain."
      footer={
        <>
          <Button variant="secondary" onClick={props.onClose}>Cancel</Button>
          <Button onClick={publish} loading={busy()} disabled={!text().trim() && !media().length}>Publish</Button>
        </>
      }
    >
      <div class="flex flex-col gap-3">
        <Textarea rows={4} value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder={props.reel ? "Caption" : "What is happening?"} maxLength={4000} />
        <Show when={!props.story}>
          <Field label="Hashtags">
            <Input value={tags()} onInput={(e) => setTags(e.currentTarget.value)} placeholder="hashgram launch" />
          </Field>
        </Show>
        <div class="flex items-center gap-2">
          <Button size="sm" variant="secondary" onClick={addMedia} loading={busy()}>
            <ImageIcon size={12} /> {props.reel ? "Choose video" : "Add media"}
          </Button>
          <For each={media()}>
            {(m) => (
              <span class="badge">
                {m.kind} {(m.size / 1024 / 1024).toFixed(1)} MB
                <button type="button" class="ml-1" onClick={() => setMedia((l) => l.filter((x) => x.cid !== m.cid))}>
                  <Trash2 size={10} />
                </button>
              </span>
            )}
          </For>
        </div>
        <Show when={props.reel}>
          <Notice>Reels are pre-encoded MP4 (H.264/AAC) up to 100 MB, chunked into 1 MiB pieces and uploaded to store/media nodes. Every chunk is hash-verified on download.</Notice>
        </Show>
        <Show when={err()}>
          <Notice strong>{err()}</Notice>
        </Show>
      </div>
    </Dialog>
  );
}
