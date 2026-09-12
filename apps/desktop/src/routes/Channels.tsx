// Channels: broadcast channels as signed CHANNEL_CREATE events; posts carry
// the channel id. Create / join (follow the owner) / post / moderate (hide
// locally via mute/block).
import { createResource, createSignal, For, Show, onMount, onCleanup } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Plus, Radio } from "lucide-solid";
import { Button, Card, Dialog, Field, Input, Notice, Skeleton, Empty, Switch, Textarea } from "~/components/ui";
import { PersonLabel, Mono } from "~/components/identity";
import { ipc, on, pick, str, type EventView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { person } from "~/lib/people";
import { PostCard, Composer } from "./Feed";

export function Channels() {
  const params = useParams<{ id?: string }>();
  const navigate = useNavigate();
  const [list, { refetch }] = createResource(() => ipc.channels().catch(() => [] as EventView[]));
  const [create, setCreate] = createSignal(false);
  const [q, setQ] = createSignal("");
  onMount(async () => {
    const un = await on("feed:changed", () => void refetch());
    onCleanup(() => un());
  });
  const filtered = () => (list() ?? []).filter((c) => !q() || str(pick(c.payload, "name")).toLowerCase().includes(q().toLowerCase()));
  return (
    <div class="flex h-full">
      <aside class="flex w-[280px] shrink-0 flex-col border-r border-border">
        <div class="flex items-center gap-2 border-b border-border px-3 py-2">
          <h1 class="text-sm font-semibold">Channels</h1>
          <span class="flex-1" />
          <Button size="icon" variant="ghost" onClick={() => setCreate(true)} title="Create channel">
            <Plus size={16} />
          </Button>
        </div>
        <div class="border-b border-border p-2">
          <Input placeholder="Search channels you follow" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
        </div>
        <div class="min-h-0 flex-1 overflow-auto">
          <Show when={!list.loading} fallback={<div class="p-3"><Skeleton lines={3} /></div>}>
            <Show when={filtered().length} fallback={<p class="p-3 text-xs text-muted">No channels yet. Channels from people you follow appear here; create your own with +.</p>}>
              <For each={filtered()}>
                {(c) => (
                  <button type="button" class={`row-hover flex w-full items-center gap-3 border-b border-border px-3 py-2.5 text-left ${params.id === c.id ? "bg-surface-2" : ""}`} onClick={() => navigate(`/channels/${c.id}`)}>
                    <Radio size={16} class="text-muted" />
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-sm font-medium">{str(pick(c.payload, "name"))}</span>
                      <PersonLabel person={person(c.author)} size="sm" />
                    </span>
                  </button>
                )}
              </For>
            </Show>
          </Show>
        </div>
      </aside>
      <section class="min-w-0 flex-1 overflow-auto">
        <Show when={params.id} fallback={<Empty title="Pick a channel">A channel is a signed event; its posts are signed events that point at it. Followers see them in order.</Empty>}>
          <ChannelView id={params.id!} meta={list()?.find((c) => c.id === params.id)} />
        </Show>
      </section>
      <CreateChannel open={create()} onClose={() => setCreate(false)} onCreated={(id) => { void refetch(); navigate(`/channels/${id}`); }} />
    </div>
  );
}

function ChannelView(props: { id: string; meta: EventView | undefined }) {
  const [posts, { refetch }] = createResource(() => props.id, (id) => ipc.channelPosts(id).catch(() => [] as EventView[]));
  const [composer, setComposer] = createSignal(false);
  const me = () => store.status()?.address ?? "";
  const canPost = () => props.meta && (props.meta.author === me() || pick(props.meta.payload, "open_posting") === true);
  onMount(async () => {
    const un = await on("feed:changed", () => void refetch());
    onCleanup(() => un());
  });
  return (
    <div class="page flex flex-col gap-4">
      <Show when={props.meta} fallback={<Card class="p-4"><Notice>Channel <Mono text={props.id} head={10} tail={6} /> is not in your cache. Follow its owner to see it.</Notice></Card>}>
        <div class="flex items-start gap-4">
          <div class="min-w-0 flex-1">
            <h1 class="page-title">{str(pick(props.meta!.payload, "name"))}</h1>
            <p class="text-sm text-muted">{str(pick(props.meta!.payload, "description"))}</p>
            <p class="mt-1 text-xs text-muted">
              by <PersonLabel person={person(props.meta!.author)} size="sm" /> · {pick(props.meta!.payload, "open_posting") === true ? "anyone can post" : "owner posts"} · {posts()?.length ?? 0} posts
            </p>
          </div>
          <Show when={canPost()}>
            <Button onClick={() => setComposer(true)}>
              <Plus size={12} /> Post
            </Button>
          </Show>
        </div>
      </Show>
      <Show when={!posts.loading} fallback={<Skeleton lines={4} />}>
        <Show when={posts()?.length} fallback={<Card><div class="p-4 text-sm text-muted">No posts in this channel yet.</div></Card>}>
          <For each={posts()}>{(p) => <PostCard ev={p} onChanged={() => void refetch()} />}</For>
        </Show>
      </Show>
      <Composer open={composer()} onClose={() => setComposer(false)} onPosted={() => void refetch()} channel={props.id} />
    </div>
  );
}

function CreateChannel(props: { open: boolean; onClose: () => void; onCreated: (id: string) => void }) {
  const [name, setName] = createSignal("");
  const [desc, setDesc] = createSignal("");
  const [openPosting, setOpenPosting] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const create = async () => {
    setBusy(true);
    try {
      const ev = await ipc.channelCreate(name(), desc(), openPosting());
      props.onCreated(ev.id);
      props.onClose();
      setName("");
      setDesc("");
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Create channel" footer={<><Button variant="secondary" onClick={props.onClose}>Cancel</Button><Button onClick={create} loading={busy()} disabled={!name().trim()}>Create</Button></>}>
      <div class="flex flex-col gap-3">
        <Field label="Name">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        </Field>
        <Field label="Description">
          <Textarea rows={3} value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} />
        </Field>
        <Switch label="Open posting" hint="Anyone who follows the channel may post into it; otherwise only you." checked={openPosting()} onChange={setOpenPosting} />
      </div>
    </Dialog>
  );
}
