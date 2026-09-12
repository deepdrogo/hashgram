// Messages: MLS conversations. Left: conversations; right: the thread with
// composer (text, files, voice), receipts, reactions, replies, disappearing
// timer, chat info (members' devices, store nodes), and the call button.
import { createEffect, createMemo, createResource, createSignal, For, Show, onCleanup, onMount } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Send, Paperclip, Mic, Square, Phone, Video, Info, Plus, Check, CheckCheck, Clock, Reply, Trash2, Pencil, Smile, RefreshCw, Users, Search } from "lucide-solid";
import { Button, Dialog, Field, Input, Notice, Skeleton, Empty, Badge, Switch } from "~/components/ui";
import { PersonLabel, Mono, Avatar } from "~/components/identity";
import { VirtualList } from "~/components/VirtualList";
import { ipc, on, type ConversationMeta, type MessageView, type ChatInfo } from "~/lib/ipc";
import { store } from "~/lib/store";
import { formatTime, isHashAddress, relTime, truncateMiddle } from "~/lib/format";
import { attachmentUrl, bytes, durationLabel } from "~/lib/media";
import { CallPanel } from "./Calls";

const REACTIONS = ["👍", "❤️", "😂", "🔥", "👀", "🙏"];
const DISAPPEAR = [
  { label: "Off", secs: 0 },
  { label: "30 s", secs: 30 },
  { label: "5 min", secs: 300 },
  { label: "1 hour", secs: 3600 },
  { label: "1 day", secs: 86400 },
  { label: "1 week", secs: 604800 },
];

export function Messages() {
  const params = useParams<{ group?: string }>();
  const navigate = useNavigate();
  const [convs, { refetch: refetchConvs }] = createResource(() => ipc.chatList().catch(() => [] as ConversationMeta[]));
  const [newOpen, setNewOpen] = createSignal(false);
  const [search, setSearch] = createSignal("");
  const [results, setResults] = createSignal<MessageView[] | null>(null);
  const me = () => store.status()?.address ?? "";
  const selected = () => params.group ?? "";

  onMount(async () => {
    const un = await on("chat:changed", () => void refetchConvs());
    onCleanup(() => un());
  });

  const runSearch = async (q: string) => {
    setSearch(q);
    if (q.trim().length < 2) return setResults(null);
    setResults(await ipc.chatSearch(q).catch(() => []));
  };

  return (
    <div class="flex h-full">
      <aside class="flex w-[300px] shrink-0 flex-col border-r border-border">
        <div class="flex items-center gap-2 border-b border-border px-3 py-2">
          <h1 class="text-sm font-semibold">Messages</h1>
          <span class="flex-1" />
          <Button size="icon" variant="ghost" title="Sync now" onClick={() => void ipc.chatSyncNow().then(() => refetchConvs())}>
            <RefreshCw size={14} />
          </Button>
          <Button size="icon" variant="ghost" title="New conversation" onClick={() => setNewOpen(true)}>
            <Plus size={16} />
          </Button>
        </div>
        <div class="border-b border-border p-2">
          <div class="flex items-center gap-2 rounded-md border border-border bg-surface-2 px-2">
            <Search size={12} class="text-muted" />
            <input class="h-8 flex-1 bg-transparent text-xs outline-none placeholder:text-muted" placeholder="Search messages (local, encrypted)" value={search()} onInput={(e) => void runSearch(e.currentTarget.value)} />
          </div>
        </div>
        <Show when={results()} fallback={<ConversationList convs={convs() ?? []} loading={convs.loading} selected={selected()} me={me()} onOpen={(g) => navigate(`/messages/${g}`)} />}>
          {(r) => (
            <div class="min-h-0 flex-1 overflow-auto">
              <Show when={r().length} fallback={<p class="p-3 text-xs text-muted">No matches.</p>}>
                <For each={r()}>
                  {(m) => (
                    <button type="button" class="row-hover w-full border-b border-border px-3 py-2 text-left" onClick={() => navigate(`/messages/${m.group_id}`)}>
                      <p class="truncate text-sm">{m.text}</p>
                      <p class="mono text-[11px] text-muted">{truncateMiddle(m.sender, 10, 4)} · {formatTime(Math.floor(m.timestamp_ms / 1000))}</p>
                    </button>
                  )}
                </For>
              </Show>
            </div>
          )}
        </Show>
      </aside>
      <section class="min-w-0 flex-1">
        <Show when={selected()} fallback={<Empty title="Pick a conversation">End-to-end encrypted with MLS. Store nodes keep only ciphertext until your devices fetch it.</Empty>}>
          <Thread groupId={selected()} me={me()} conv={convs()?.find((c) => c.group_id === selected())} onChanged={() => void refetchConvs()} />
        </Show>
      </section>
      <NewConversation open={newOpen()} onClose={() => setNewOpen(false)} onCreated={(g) => { void refetchConvs(); navigate(`/messages/${g}`); }} />
    </div>
  );
}

function ConversationList(props: { convs: ConversationMeta[]; loading: boolean; selected: string; me: string; onOpen: (g: string) => void }) {
  return (
    <div class="min-h-0 flex-1 overflow-auto">
      <Show when={!props.loading} fallback={<div class="p-3"><Skeleton lines={4} /></div>}>
        <Show when={props.convs.length} fallback={<p class="p-3 text-xs text-muted">No conversations yet. Start one with +.</p>}>
          <For each={props.convs}>
            {(c) => {
              const other = () => c.members.find((m) => m !== props.me) ?? props.me;
              return (
                <button type="button" class={`row-hover flex w-full items-center gap-3 border-b border-border px-3 py-2.5 text-left ${props.selected === c.group_id ? "bg-surface-2" : ""}`} onClick={() => props.onOpen(c.group_id)}>
                  <Avatar address={c.direct ? other() : c.group_id} size={36} />
                  <span class="min-w-0 flex-1">
                    <span class="flex items-baseline justify-between gap-2">
                      <Show when={c.direct} fallback={<span class="truncate text-sm font-medium">{c.name}</span>}>
                        <PersonLabel person={{ address: other() }} size="sm" />
                      </Show>
                      <span class="mono shrink-0 text-[10px] text-muted">{c.last_ts ? relTime(Math.floor(c.last_ts / 1000)) : ""}</span>
                    </span>
                    <span class="flex items-center justify-between gap-2">
                      <span class="truncate text-xs text-muted">{c.last_preview || (c.direct ? "" : `${c.members.length} members`)}</span>
                      <Show when={c.unread > 0}>
                        <span class="badge-strong">{c.unread}</span>
                      </Show>
                    </span>
                  </span>
                </button>
              );
            }}
          </For>
        </Show>
      </Show>
    </div>
  );
}

function Thread(props: { groupId: string; me: string; conv: ConversationMeta | undefined; onChanged: () => void }) {
  const [history, { refetch }] = createResource(() => props.groupId, (g) => ipc.chatHistory(g, undefined, 200).catch(() => [] as MessageView[]));
  const [text, setText] = createSignal("");
  const [replyTo, setReplyTo] = createSignal<MessageView | null>(null);
  const [editing, setEditing] = createSignal<MessageView | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [info, setInfo] = createSignal(false);
  const [call, setCall] = createSignal<{ video: boolean } | null>(null);
  const [typing, setTyping] = createSignal<string[]>([]);
  const [recording, setRecording] = createSignal<MediaRecorder | null>(null);
  const [recStart, setRecStart] = createSignal(0);
  let bottom!: HTMLDivElement;
  let lastTyping = 0;

  onMount(async () => {
    const un = await on("chat:changed", (p) => {
      if (!p.group_id || p.group_id === props.groupId) void refetch();
    });
    const t = setInterval(() => void ipc.chatTypingIn(props.groupId).then(setTyping).catch(() => undefined), 3000);
    onCleanup(() => {
      un();
      clearInterval(t);
    });
  });
  createEffect(() => {
    props.groupId;
    void ipc.chatMarkRead(props.groupId).then(props.onChanged).catch(() => undefined);
  });
  createEffect(() => {
    if (history()) queueMicrotask(() => bottom?.scrollIntoView({ block: "end" }));
  });

  const send = async () => {
    const t = text().trim();
    if (!t) return;
    setBusy(true);
    try {
      if (editing()) {
        await ipc.chatEdit(props.groupId, editing()!.id, t);
        setEditing(null);
      } else {
        await ipc.chatSendText(props.groupId, t, replyTo()?.id);
      }
      setText("");
      setReplyTo(null);
      await refetch();
      props.onChanged();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const attach = async () => {
    const path = await openDialog({ multiple: false, title: "Attach a file" });
    if (!path || typeof path !== "string") return;
    setBusy(true);
    try {
      await ipc.chatSendFile(props.groupId, path, text().trim() || undefined);
      setText("");
      await refetch();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const toggleVoice = async () => {
    const rec = recording();
    if (rec) {
      rec.stop();
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const r = new MediaRecorder(stream, { mimeType: "audio/webm" });
      const chunks: Blob[] = [];
      r.ondataavailable = (e) => chunks.push(e.data);
      r.onstop = async () => {
        stream.getTracks().forEach((tr) => tr.stop());
        setRecording(null);
        const blob = new Blob(chunks, { type: "audio/webm" });
        const buf = new Uint8Array(await blob.arrayBuffer());
        let bin = "";
        for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
        const b64 = btoa(bin);
        setBusy(true);
        try {
          await ipc.chatSendVoice(props.groupId, b64, Date.now() - recStart());
          await refetch();
        } catch (e) {
          store.toast(String(e), "error");
        } finally {
          setBusy(false);
        }
      };
      r.start();
      setRecStart(Date.now());
      setRecording(r);
    } catch (e) {
      store.toast(`Microphone: ${String(e)}`, "error");
    }
  };
  const onType = (v: string) => {
    setText(v);
    const now = Date.now();
    if (now - lastTyping > 5000 && v.length > 0) {
      lastTyping = now;
      void ipc.chatTyping(props.groupId).catch(() => undefined);
    }
  };
  const byId = createMemo(() => new Map((history() ?? []).map((m) => [m.id, m])));
  const others = () => (props.conv?.members ?? []).filter((m) => m !== props.me);

  return (
    <div class="flex h-full flex-col">
      <header class="flex items-center gap-3 border-b border-border px-4 py-2">
        <Show when={props.conv}>
          <Show when={props.conv!.direct} fallback={<span class="text-sm font-medium">{props.conv!.name} <span class="text-xs text-muted">· {props.conv!.members.length} members</span></span>}>
            <PersonLabel person={{ address: others()[0] ?? props.me }} copy />
          </Show>
        </Show>
        <Show when={props.conv?.disappear_secs}>
          <Badge title="Disappearing messages">
            <Clock size={10} /> {DISAPPEAR.find((d) => d.secs === props.conv!.disappear_secs)?.label ?? `${props.conv!.disappear_secs}s`}
          </Badge>
        </Show>
        <span class="flex-1" />
        <Show when={props.conv?.direct}>
          <Button size="icon" variant="ghost" title="Voice call" onClick={() => setCall({ video: false })}>
            <Phone size={16} />
          </Button>
          <Button size="icon" variant="ghost" title="Video call" onClick={() => setCall({ video: true })}>
            <Video size={16} />
          </Button>
        </Show>
        <Show when={!props.conv?.direct}>
          <Button size="icon" variant="ghost" title="Group calls need an SFU node announced on the network; none is. Group SFU calls are not end-to-end encrypted against the SFU operator." disabled>
            <Phone size={16} />
          </Button>
        </Show>
        <Button size="icon" variant="ghost" title="Chat info" onClick={() => setInfo(true)}>
          <Info size={16} />
        </Button>
      </header>
      <div class="min-h-0 flex-1 overflow-auto px-4 py-3">
        <Show when={!history.loading} fallback={<Skeleton lines={5} />}>
          <Show when={history()?.length} fallback={<p class="py-8 text-center text-xs text-muted">No messages yet. Everything you send here is end-to-end encrypted.</p>}>
            <For each={history()}>
              {(m) => <Bubble m={m} me={props.me} byId={byId()} onReply={() => setReplyTo(m)} onEdit={() => { setEditing(m); setText(m.text); }} onDelete={() => void ipc.chatDelete(props.groupId, m.id).then(() => refetch())} onReact={(r) => void ipc.chatReact(props.groupId, m.id, r).then(() => refetch())} />}
            </For>
          </Show>
        </Show>
        <div ref={bottom} />
      </div>
      <Show when={typing().length}>
        <p class="px-4 pb-1 text-[11px] text-muted">{typing().map((t) => truncateMiddle(t, 10, 4)).join(", ")} typing…</p>
      </Show>
      <Show when={replyTo() || editing()}>
        <div class="flex items-center gap-2 border-t border-border px-4 py-1.5 text-xs text-muted">
          {editing() ? <Pencil size={12} /> : <Reply size={12} />}
          <span class="truncate">{editing() ? "Editing" : "Replying to"}: {(editing() ?? replyTo())!.text}</span>
          <button type="button" class="ml-auto hover:text-fg" onClick={() => { setReplyTo(null); setEditing(null); setText(""); }}>
            cancel
          </button>
        </div>
      </Show>
      <footer class="flex items-end gap-2 border-t border-border px-3 py-2">
        <Button size="icon" variant="ghost" title="Attach file (encrypted)" onClick={attach} disabled={busy()}>
          <Paperclip size={16} />
        </Button>
        <textarea
          class="textarea min-h-9 max-h-40 flex-1 py-2"
          rows={1}
          placeholder="Message"
          value={text()}
          onInput={(e) => onType(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <Button size="icon" variant={recording() ? "primary" : "ghost"} title={recording() ? "Stop and send voice note" : "Record voice note"} onClick={toggleVoice} disabled={busy()}>
          {recording() ? <Square size={16} /> : <Mic size={16} />}
        </Button>
        <Button size="icon" title="Send" onClick={send} loading={busy()} disabled={!text().trim()}>
          <Send size={16} />
        </Button>
      </footer>
      <ChatInfoDialog open={info()} onClose={() => setInfo(false)} groupId={props.groupId} conv={props.conv} me={props.me} onChanged={props.onChanged} />
      <Show when={call()}>
        <CallPanel groupId={props.groupId} peer={others()[0] ?? ""} video={call()!.video} onClose={() => setCall(null)} />
      </Show>
    </div>
  );
}

function Bubble(props: { m: MessageView; me: string; byId: Map<string, MessageView>; onReply: () => void; onEdit: () => void; onDelete: () => void; onReact: (r: string) => void }) {
  const m = () => props.m;
  const mine = () => m().outgoing;
  const [pick, setPick] = createSignal(false);
  const replied = () => (m().reply_to ? props.byId.get(m().reply_to) : undefined);
  return (
    <div class={`group mb-2 flex ${mine() ? "justify-end" : "justify-start"}`}>
      <div class={`max-w-[70%] rounded-lg border px-3 py-2 text-sm ${mine() ? "border-accent bg-surface-2" : "border-border bg-surface"}`}>
        <Show when={!mine()}>
          <div class="mb-0.5">
            <PersonLabel person={{ address: m().sender }} size="sm" />
          </div>
        </Show>
        <Show when={replied()}>
          <div class="mb-1 border-l-2 border-accent pl-2 text-xs text-muted">
            <span class="truncate">{replied()!.text || `[${replied()!.kind.toLowerCase()}]`}</span>
          </div>
        </Show>
        <Show when={m().kind === "DELETE"}>
          <p class="text-xs italic text-muted">message deleted</p>
        </Show>
        <Show when={m().kind === "CALL"}>
          <p class="text-xs text-muted">
            <Phone size={12} class="mr-1 inline" /> call {m().call_kind}
          </p>
        </Show>
        <Show when={m().text}>
          <p class="selectable whitespace-pre-wrap break-words">{m().text}</p>
        </Show>
        <For each={m().attachments}>{(a) => <AttachmentBubble a={a} />}</For>
        <div class="mt-1 flex items-center gap-2 text-[10px] text-muted">
          <span class="mono">{new Date(m().timestamp_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>
          <Show when={mine()}>
            <span title={m().state}>{m().state === "read" ? <CheckCheck size={12} /> : m().state === "delivered" ? <CheckCheck size={12} class="opacity-50" /> : <Check size={12} />}</span>
          </Show>
          <Show when={!m().this_device && mine()}>
            <span title={`sent from another device (${m().sender_device.slice(0, 8)}…)`}>· other device</span>
          </Show>
          <Show when={m().disappear_after_secs}>
            <Clock size={10} />
          </Show>
          <span class="flex-1" />
          <span class="hidden gap-1 group-hover:flex">
            <button type="button" class="hover:text-fg" title="React" onClick={() => setPick((v) => !v)}>
              <Smile size={12} />
            </button>
            <button type="button" class="hover:text-fg" title="Reply" onClick={props.onReply}>
              <Reply size={12} />
            </button>
            <Show when={mine() && m().kind === "TEXT"}>
              <button type="button" class="hover:text-fg" title="Edit" onClick={props.onEdit}>
                <Pencil size={12} />
              </button>
              <button type="button" class="hover:text-fg" title="Delete for everyone" onClick={props.onDelete}>
                <Trash2 size={12} />
              </button>
            </Show>
          </span>
        </div>
        <Show when={pick()}>
          <div class="mt-1 flex gap-1">
            <For each={REACTIONS}>
              {(r) => (
                <button type="button" class="rounded px-1 hover:bg-border" onClick={() => { props.onReact(r); setPick(false); }}>
                  {r}
                </button>
              )}
            </For>
          </div>
        </Show>
        <Show when={Object.keys(m().reactions).length}>
          <div class="mt-1 flex flex-wrap gap-1">
            <For each={Object.entries(m().reactions)}>
              {([r, who]) => (
                <span class="badge" title={who.map((w) => truncateMiddle(w, 8, 4)).join(", ")}>
                  {r} {who.length}
                </span>
              )}
            </For>
          </div>
        </Show>
      </div>
    </div>
  );
}

function AttachmentBubble(props: { a: MessageView["attachments"][number] }) {
  const [url] = createResource(() => props.a, (a) => attachmentUrl(a).catch((e) => { store.toast(String(e), "error"); return ""; }));
  return (
    <div class="mt-1">
      <Show when={url()} fallback={<div class="skeleton h-24 w-48" />}>
        <Show when={props.a.kind === "image"}>
          <img src={url()} alt={props.a.name} class="max-h-72 rounded-md" />
        </Show>
        <Show when={props.a.kind === "video"}>
          <video src={url()} controls class="max-h-72 rounded-md" />
        </Show>
        <Show when={props.a.kind === "audio"}>
          <div class="flex items-center gap-2">
            <audio src={url()} controls class="h-8" />
            <span class="mono text-[10px] text-muted">{durationLabel(props.a.duration_ms)}</span>
          </div>
        </Show>
        <Show when={props.a.kind === "file"}>
          <a href={url()} download={props.a.name} class="flex items-center gap-2 rounded-md border border-border px-2 py-1 text-xs">
            <Paperclip size={12} /> {props.a.name} <span class="text-muted">{bytes(props.a.size)}</span>
          </a>
        </Show>
      </Show>
      <p class="mono mt-0.5 text-[10px] text-muted" title={props.a.cid}>hash-verified · {truncateMiddle(props.a.cid, 8, 6)}</p>
    </div>
  );
}

function ChatInfoDialog(props: { open: boolean; onClose: () => void; groupId: string; conv: ConversationMeta | undefined; me: string; onChanged: () => void }) {
  const [info, { refetch }] = createResource(() => (props.open ? props.groupId : null), (g) => ipc.chatInfo(g).catch(() => null as ChatInfo | null));
  const [addr, setAddr] = createSignal("");
  const setDisappear = async (secs: number) => {
    await ipc.chatSetDisappear(props.groupId, secs).catch((e) => store.toast(String(e), "error"));
    props.onChanged();
    void refetch();
  };
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Chat info" width="max-w-xl">
      <Show when={info()} fallback={<Skeleton lines={4} />}>
        {(i) => (
          <div class="flex flex-col gap-4 text-sm">
            <div>
              <p class="label">Members and their devices (from chain)</p>
              <ul class="space-y-1">
                <For each={i().members}>
                  {([address, device, label]) => (
                    <li class="flex items-center justify-between gap-2">
                      <PersonLabel person={{ address }} size="sm" />
                      <span class="mono text-xs text-muted">{label || truncateMiddle(device, 8, 6)}{address === props.me ? " (you)" : ""}</span>
                      <Show when={!props.conv?.direct && address !== props.me}>
                        <Button size="sm" variant="ghost" onClick={() => void ipc.chatRemoveMember(props.groupId, address).then(() => { void refetch(); props.onChanged(); })}>
                          Remove
                        </Button>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </div>
            <Show when={!props.conv?.direct}>
              <Field label="Add a member by address">
                <div class="flex gap-2">
                  <Input mono value={addr()} onInput={(e) => setAddr(e.currentTarget.value)} placeholder="hash1…" />
                  <Button variant="secondary" disabled={!isHashAddress(addr())} onClick={() => void ipc.chatAddMember(props.groupId, addr().trim()).then(() => { setAddr(""); void refetch(); props.onChanged(); }).catch((e) => store.toast(String(e), "error"))}>
                    Add
                  </Button>
                </div>
              </Field>
            </Show>
            <div>
              <p class="label">Disappearing messages (applies to what you send)</p>
              <div class="flex flex-wrap gap-1">
                <For each={DISAPPEAR}>
                  {(d) => (
                    <Button size="sm" variant={i().disappear_secs === d.secs ? "primary" : "secondary"} onClick={() => void setDisappear(d.secs)}>
                      {d.label}
                    </Button>
                  )}
                </For>
              </div>
              <p class="mt-1 text-xs text-muted">Timers are honoured by every device in the chat; nodes never see them.</p>
            </div>
            <div>
              <p class="label">Store nodes holding your mailbox</p>
              <Show when={i().store_nodes.length} fallback={<p class="text-xs text-muted">None found in the DHT yet; messages are also offered to every connected store node.</p>}>
                <ul class="mono space-y-0.5 text-xs">
                  <For each={i().store_nodes}>{(p) => <li>{p}</li>}</For>
                </ul>
              </Show>
              <p class="mt-1 text-xs text-muted">Last mailbox sync: {i().last_sync_secs !== null ? `${i().last_sync_secs}s ago` : "not yet"}.</p>
            </div>
            <Notice>
              <Users size={12} class="mr-1 inline" /> Group id <Mono text={props.groupId} head={10} tail={6} copy />. Every message is encrypted for each member device; a display name in a message is never identity — the verified handle is.
            </Notice>
          </div>
        )}
      </Show>
    </Dialog>
  );
}

function NewConversation(props: { open: boolean; onClose: () => void; onCreated: (g: string) => void }) {
  const [mode, setMode] = createSignal<"direct" | "group">("direct");
  const [addr, setAddr] = createSignal("");
  const [name, setName] = createSignal("");
  const [members, setMembers] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const memberList = () => members().split(/[\s,]+/).map((m) => m.trim()).filter(Boolean);
  const create = async () => {
    setBusy(true);
    setErr(null);
    try {
      let target = addr().trim();
      if (target.startsWith("@")) {
        const r = await ipc.searchResolve(target);
        if (r.kind !== "username") throw new Error("no such username on chain");
        target = r.address;
      }
      const g = mode() === "direct" ? await ipc.chatStartDirect(target) : await ipc.chatCreateGroup(name(), memberList());
      props.onCreated(g);
      props.onClose();
      setAddr("");
      setName("");
      setMembers("");
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
      title="New conversation"
      description="Recipients are found by address or on-chain @username; their devices come from the chain."
      footer={
        <>
          <Button variant="secondary" onClick={props.onClose}>Cancel</Button>
          <Button onClick={create} loading={busy()} disabled={mode() === "direct" ? !addr().trim() : !name().trim() || !memberList().every(isHashAddress) || !memberList().length}>
            Start
          </Button>
        </>
      }
    >
      <div class="flex flex-col gap-3">
        <div class="flex gap-2">
          <Button size="sm" variant={mode() === "direct" ? "primary" : "secondary"} onClick={() => setMode("direct")}>Direct</Button>
          <Button size="sm" variant={mode() === "group" ? "primary" : "secondary"} onClick={() => setMode("group")}>Group</Button>
        </div>
        <Show when={mode() === "direct"}>
          <Field label="Address or @username">
            <Input mono value={addr()} onInput={(e) => setAddr(e.currentTarget.value)} placeholder="hash1… or @name" onKeyDown={(e) => e.key === "Enter" && void create()} />
          </Field>
        </Show>
        <Show when={mode() === "group"}>
          <Field label="Group name">
            <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} />
          </Field>
          <Field label="Member addresses (space or comma separated)">
            <Input mono value={members()} onInput={(e) => setMembers(e.currentTarget.value)} placeholder="hash1… hash1…" />
          </Field>
        </Show>
        <Show when={err()}>
          <Notice strong>{err()}</Notice>
        </Show>
        <Notice>If someone cannot be reached, they have not published a key package yet — they need to open Hashgram once while online.</Notice>
      </div>
    </Dialog>
  );
}
