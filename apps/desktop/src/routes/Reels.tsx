// Reels: vertical full-screen video, autoplay, mute, like/comment/share.
// Downloads are hash-verified per chunk in Rust; a bad provider is dropped.
import { createResource, createSignal, For, Show, onMount, onCleanup } from "solid-js";
import { Heart, MessageCircle, Share2, Volume2, VolumeX, Plus } from "lucide-solid";
import { Button, Empty, Skeleton } from "~/components/ui";
import { PersonLabel } from "~/components/identity";
import { ipc, on, pick, str, type EventView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { person } from "~/lib/people";
import { mediaUrl } from "~/lib/media";
import { copyText } from "~/lib/clipboard";
import { Composer } from "./Feed";

export function Reels() {
  const [page, { refetch }] = createResource(() => ipc.feed(["REEL_CREATE"], undefined, undefined, 100).catch(() => null));
  const [muted, setMuted] = createSignal(true);
  const [composer, setComposer] = createSignal(false);
  onMount(async () => {
    const un = await on("feed:changed", () => void refetch());
    onCleanup(() => un());
  });
  return (
    <div class="relative h-full">
      <div class="absolute right-4 top-3 z-10 flex gap-2">
        <Button size="sm" variant="secondary" onClick={() => setMuted((v) => !v)}>
          {muted() ? <VolumeX size={12} /> : <Volume2 size={12} />} {muted() ? "Muted" : "Sound"}
        </Button>
        <Button size="sm" onClick={() => setComposer(true)}>
          <Plus size={12} /> Reel
        </Button>
      </div>
      <Show when={!page.loading} fallback={<div class="p-6"><Skeleton lines={5} /></div>}>
        <Show when={page()?.events.length} fallback={<Empty title="No reels yet">Reels from people you follow appear here. Publish one with the button above (MP4 up to 100 MB).</Empty>}>
          <div class="h-full snap-y snap-mandatory overflow-y-auto">
            <For each={page()!.events}>{(e) => <Reel ev={e} muted={muted()} onChanged={() => void refetch()} />}</For>
          </div>
        </Show>
      </Show>
      <Composer open={composer()} onClose={() => setComposer(false)} onPosted={() => void refetch()} reel />
    </div>
  );
}

function Reel(props: { ev: EventView; muted: boolean; onChanged: () => void }) {
  const video = () => props.ev.media.find((m) => m.kind === "video");
  const [url] = createResource(video, (m) => (m ? mediaUrl(m).catch((e) => { store.toast(String(e), "error"); return ""; }) : Promise.resolve("")));
  const [comment, setComment] = createSignal("");
  const [showComments, setShowComments] = createSignal(false);
  let el!: HTMLVideoElement;
  let io: IntersectionObserver | null = null;
  onMount(() => {
    io = new IntersectionObserver(
      (entries) => {
        for (const en of entries) {
          if (en.isIntersecting && (store.settings()?.media.autoplay ?? true)) void el.play().catch(() => undefined);
          else el.pause();
        }
      },
      { threshold: 0.6 },
    );
    if (el) io.observe(el);
    onCleanup(() => io?.disconnect());
  });
  const likes = () => Object.values(props.ev.reactions).reduce((a, b) => a + b, 0);
  return (
    <section class="flex h-full snap-start items-center justify-center bg-bg">
      <div class="relative h-[92%] aspect-[9/16] max-w-full overflow-hidden rounded-lg border border-border bg-surface">
        <Show when={url()} fallback={<div class="skeleton h-full w-full" />}>
          <video ref={el} src={url()} loop playsinline muted={props.muted} class="h-full w-full object-cover" onClick={() => (el.paused ? void el.play() : el.pause())} />
        </Show>
        <div class="absolute inset-x-0 bottom-0 flex items-end gap-3 bg-bg/70 p-3">
          <div class="min-w-0 flex-1">
            <PersonLabel person={person(props.ev.author)} />
            <p class="selectable mt-1 text-sm">{str(pick(props.ev.payload, "caption"))}</p>
            <p class="mono mt-1 text-[10px] text-muted">hash-verified · {video()?.cid.slice(0, 12)}…</p>
          </div>
          <div class="flex flex-col items-center gap-3 text-xs">
            <button type="button" class="flex flex-col items-center hover:text-fg" onClick={() => void ipc.socialReact(props.ev.id, props.ev.my_reaction ? "" : "❤️").then(props.onChanged)}>
              <Heart size={20} fill={props.ev.my_reaction ? "#ffffff" : "none"} /> {likes()}
            </button>
            <button type="button" class="flex flex-col items-center hover:text-fg" onClick={() => setShowComments((v) => !v)}>
              <MessageCircle size={20} /> {props.ev.comments}
            </button>
            <button type="button" class="flex flex-col items-center hover:text-fg" onClick={() => void copyText(`hashgram://post/${props.ev.id}`, "Link copied")}>
              <Share2 size={20} />
            </button>
          </div>
        </div>
        <Show when={showComments()}>
          <div class="absolute inset-x-0 top-0 max-h-1/2 overflow-auto bg-bg/90 p-3 text-sm">
            <div class="flex gap-2">
              <input class="input" placeholder="Comment" value={comment()} onInput={(e) => setComment(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void ipc.commentCreate(props.ev.id, comment()).then(() => { setComment(""); props.onChanged(); })} />
            </div>
          </div>
        </Show>
      </div>
    </section>
  );
}
