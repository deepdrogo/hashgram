// The reading pane: headers with the authenticated sender, External and
// BCC badges, text body linkified (or HTML in a sandbox on request),
// attachments with Save/Open, live-attachment version state, receipts.
import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Reply, ReplyAll, Forward, Archive, Trash2, Star, ShieldCheck, ShieldAlert, Paperclip, Download, ExternalLink, Check, MailOpen } from "lucide-solid";
import { Button, Badge, Notice } from "~/components/ui";
import { Who, Mono } from "~/components/identity";
import { HtmlSandbox } from "~/components/HtmlSandbox";
import { ipc, errText, type MailView, type AttachmentView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatMs, formatBytes, handle } from "~/lib/format";
import { linkify } from "~/lib/linkify";
import { pickSavePath, confirm } from "~/lib/dialogs";

export function TextBody(props: { text: string }) {
  const parts = createMemo(() => linkify(props.text));
  return (
    <div class="prose-mail">
      <For each={parts()}>
        {(p) =>
          p.kind === "link" ? (
            <a
              href={p.value}
              onClick={async (e) => {
                e.preventDefault();
                if (await confirm(`Open in your browser?\n\n${p.value}`)) void openUrl(p.value).catch((err) => store.toast(String(err), "error"));
              }}
            >
              {p.value}
            </a>
          ) : (
            p.value
          )
        }
      </For>
    </div>
  );
}

function Attachment(props: { mail: MailView; a: AttachmentView; liveVersions: Record<string, number> }) {
  const [busy, setBusy] = createSignal(false);
  const newer = () => props.a.kind === "drive" && props.a.live && (props.liveVersions[props.a.share_id] ?? 0) > props.a.version_no;
  const save = async () => {
    const p = await pickSavePath(props.a.name);
    if (!p) return;
    setBusy(true);
    try {
      await ipc.mailAttachmentSave(props.mail.id, props.a.index, p);
      store.toast(`Saved ${props.a.name}`);
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const open = async () => {
    setBusy(true);
    try {
      await ipc.mailAttachmentOpen(props.mail.id, props.a.index);
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="card flex items-center gap-2 px-2.5 py-1.5 text-xs">
      <Paperclip size={12} class="text-muted" />
      <span class="min-w-0 flex-1 truncate" title={`${props.a.mime} · ${props.a.plaintext_hash.slice(0, 16)}…`}>
        {props.a.name}
      </span>
      <span class="tnum text-muted">{formatBytes(props.a.size)}</span>
      <Show when={props.a.kind === "drive"}>
        <Badge brand={props.a.live} title={props.a.live ? "follows the owner's edits" : "fixed version"}>
          {props.a.live ? `${t("mail_live")} v${props.a.version_no}` : `Drive v${props.a.version_no}`}
        </Badge>
        <Show when={newer()}>
          <Badge strong title={`version ${props.liveVersions[props.a.share_id]} available — open Drive → Shared with me`}>
            {t("mail_updated")}
          </Badge>
        </Show>
      </Show>
      <Button variant="ghost" size="sm" onClick={save} disabled={busy()} title="Save as…">
        <Download size={12} />
      </Button>
      <Button variant="ghost" size="sm" onClick={open} disabled={busy() || props.a.folder} title="Open with the default app">
        <ExternalLink size={12} />
      </Button>
    </div>
  );
}

export function Message(props: { m: MailView; liveVersions: Record<string, number>; onReply: (all: boolean) => void; onForward: () => void; expanded?: boolean }) {
  const [showHtml, setShowHtml] = createSignal(false);
  const receipts = createMemo(() => {
    const d = Object.entries(props.m.delivered_to);
    const r = Object.entries(props.m.read_by);
    return { d, r };
  });
  return (
    <article class="border-b border-border px-5 py-4 last:border-b-0">
      <header class="flex flex-col gap-1.5">
        <div class="flex items-start gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-[13px] font-semibold">
                <Who address={props.m.authenticated_sender} />
              </span>
              <Show when={props.m.sender_matches && !props.m.external} fallback={null}>
                <span class="inline-flex items-center gap-1 text-[11px] text-muted" title={t("mail_verified_device")}>
                  <ShieldCheck size={11} class="text-brand" />
                </span>
              </Show>
              <Show when={!props.m.sender_matches && !props.m.external}>
                <Badge strong title={t("mail_impersonation")}>
                  <ShieldAlert size={10} class="mr-1" />
                  From header differs: {handle(props.m.from.address, props.m.from.username, props.m.from.display_name)}
                </Badge>
              </Show>
              <Show when={props.m.external}>
                <Badge strong title={props.m.external?.auth_results.join(", ") || "no authentication results"}>
                  {t("mail_external")}
                </Badge>
              </Show>
              <Show when={props.m.bcc_copy}>
                <Badge title="You received a blind copy; the other recipients do not see you.">{t("mail_bcc")}</Badge>
              </Show>
              <Show when={props.m.importance === 2}>
                <Badge>high importance</Badge>
              </Show>
            </div>
            <div class="mt-0.5 text-xs text-muted">
              {t("mail_to")}:{" "}
              <For each={props.m.to}>
                {(a, i) => (
                  <>
                    <Who address={a.address} size="sm" />
                    {i() < props.m.to.length - 1 ? ", " : ""}
                  </>
                )}
              </For>
              <Show when={props.m.cc.length}>
                {" · "}
                {t("mail_cc")}:{" "}
                <For each={props.m.cc}>
                  {(a, i) => (
                    <>
                      <Who address={a.address} size="sm" />
                      {i() < props.m.cc.length - 1 ? ", " : ""}
                    </>
                  )}
                </For>
              </Show>
            </div>
            <Show when={props.m.external}>
              {(x) => (
                <div class="mt-1 text-[11px] text-muted">
                  From header: <span class="mono selectable">{x().from_header || "—"}</span>
                  <Show when={x().auth_results.length}> · {x().auth_results.join(" ")}</Show>
                  <Show when={x().gateway}>
                    {" · gateway "}
                    <Mono text={x().gateway} head={8} tail={4} />
                  </Show>
                </div>
              )}
            </Show>
          </div>
          <div class="flex shrink-0 items-center gap-1">
            <span class="tnum mr-2 text-[11px] text-muted" title={`sent ${formatMs(props.m.created_at_ms)} · received ${formatMs(props.m.received_at_ms)}`}>
              {formatMs(props.m.created_at_ms)}
            </span>
            <Button variant="ghost" size="icon-sm" title={`${t("mail_reply")} (r)`} onClick={() => props.onReply(false)}>
              <Reply size={14} />
            </Button>
            <Button variant="ghost" size="icon-sm" title={`${t("mail_reply_all")} (a)`} onClick={() => props.onReply(true)}>
              <ReplyAll size={14} />
            </Button>
            <Button variant="ghost" size="icon-sm" title={`${t("mail_forward")} (f)`} onClick={props.onForward}>
              <Forward size={14} />
            </Button>
          </div>
        </div>
      </header>
      <div class="mt-3">
        <Show when={props.m.body_html && showHtml()} fallback={<TextBody text={props.m.body_text} />}>
          <HtmlSandbox html={props.m.body_html} />
        </Show>
        <Show when={props.m.body_html}>
          <button type="button" class="mt-2 text-[11px] text-muted hover:text-fg" onClick={() => setShowHtml((v) => !v)}>
            {showHtml() ? "Show plain text" : "Show HTML version (sandboxed: no scripts, no remote images)"}
          </button>
        </Show>
      </div>
      <Show when={props.m.attachments.length}>
        <div class="mt-3 flex flex-col gap-1.5">
          <For each={props.m.attachments}>{(a) => <Attachment mail={props.m} a={a} liveVersions={props.liveVersions} />}</For>
        </div>
      </Show>
      <Show when={props.m.outgoing && (receipts().d.length || receipts().r.length)}>
        <div class="mt-3 flex flex-wrap gap-3 text-[11px] text-muted">
          <For each={receipts().d}>
            {([a, ms]) => (
              <span class="inline-flex items-center gap-1" title={formatMs(ms)}>
                <Check size={11} /> {t("mail_delivered")}: <Who address={a} size="sm" />
              </span>
            )}
          </For>
          <For each={receipts().r}>
            {([a, ms]) => (
              <span class="inline-flex items-center gap-1" title={formatMs(ms)}>
                <MailOpen size={11} /> {t("mail_read_by")}: <Who address={a} size="sm" />
              </span>
            )}
          </For>
        </div>
      </Show>
      <Show when={props.m.expire_after_secs > 0}>
        <p class="mt-2 text-[11px] text-muted">This message is set to expire {Math.round(props.m.expire_after_secs / 3600)} h after being read.</p>
      </Show>
    </article>
  );
}

export function Reader(props: {
  id: string | null;
  folder: string;
  threaded: boolean;
  onReply: (id: string, all: boolean) => void;
  onForward: (id: string) => void;
  onChanged: () => void;
}) {
  const [msg, { refetch }] = createResource(
    () => (props.id ? { id: props.id, tick: store.ticks().mail } : null),
    async ({ id }) => {
      const m = await ipc.mailGet(id);
      if (!m) return null;
      const thread = props.threaded ? await ipc.mailThread(m.thread_id).catch(() => null) : null;
      return { m, thread };
    },
  );
  const [liveVersions] = createResource(
    () => store.ticks().drive,
    () => ipc.mailLiveAttachmentVersions().catch(() => ({}) as Record<string, number>),
  );
  // Mark read after the configured delay.
  let readTimer: ReturnType<typeof setTimeout> | null = null;
  const armRead = (m: MailView) => {
    if (readTimer) clearTimeout(readTimer);
    if (m.read || m.outgoing) return;
    const secs = store.settings()?.mail.mark_read_after_secs ?? 0;
    readTimer = setTimeout(() => {
      void ipc.mailMarkRead(m.id, true).then(() => {
        store.refreshCounts();
        props.onChanged();
      });
    }, secs * 1000);
  };
  const messages = createMemo(() => {
    const d = msg();
    if (!d) return [] as MailView[];
    if (d.thread && d.thread.messages.length) return d.thread.messages;
    return [d.m];
  });
  const current = () => msg()?.m ?? null;
  createMemo(() => {
    const c = current();
    if (c) armRead(c);
  });

  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      props.onChanged();
      store.refreshCounts();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };

  return (
    <Show when={props.id} fallback={<div class="flex h-full items-center justify-center text-sm text-muted">{t("mail_select")}</div>}>
      <Show when={msg()} fallback={<div class="p-6 text-sm text-muted">{msg.loading ? t("loading") : "Message not found"}</div>}>
        {(d) => (
          <div class="flex h-full flex-col">
            <div class="flex h-10 shrink-0 items-center gap-1 border-b border-border px-3">
              <h2 class="min-w-0 flex-1 truncate text-[13px] font-semibold">{d().m.subject || "(no subject)"}</h2>
              <Show when={d().m.folder === "requests"}>
                <Button size="sm" variant="brand" onClick={() => act(() => ipc.mailAcceptRequest(d().m.id), "Moved to Inbox")}>
                  {t("mail_accept")}
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  onClick={async () => {
                    if (await confirm(`Block ${d().m.authenticated_sender}? Everything from this address is dropped from now on.`))
                      await act(async () => {
                        await ipc.peopleBlock(d().m.authenticated_sender);
                        await ipc.mailTrash(d().m.id);
                      }, "Sender blocked");
                  }}
                >
                  {t("mail_block")}
                </Button>
              </Show>
              <Button variant="ghost" size="icon-sm" title={`${d().m.starred ? t("mail_unstar") : t("mail_star")} (s)`} onClick={() => act(() => ipc.mailStar(d().m.id, !d().m.starred))}>
                <Star size={14} fill={d().m.starred ? "currentColor" : "none"} class={d().m.starred ? "text-brand" : ""} />
              </Button>
              <Show when={!d().m.read}>
                <Button variant="ghost" size="icon-sm" title="Mark read" onClick={() => act(() => ipc.mailMarkRead(d().m.id, true))}>
                  <MailOpen size={14} />
                </Button>
              </Show>
              <Show when={d().m.folder !== "archive"}>
                <Button variant="ghost" size="icon-sm" title={`${t("mail_archive_action")} (e)`} onClick={() => act(() => ipc.mailArchive(d().m.id), "Archived")}>
                  <Archive size={14} />
                </Button>
              </Show>
              <Button
                variant="ghost"
                size="icon-sm"
                title={`${d().m.folder === "trash" ? "Delete permanently" : t("mail_trash_action")} (#)`}
                onClick={async () => {
                  if (d().m.folder === "trash" && !(await confirm("Delete this message permanently?"))) return;
                  await act(() => ipc.mailTrash(d().m.id), d().m.folder === "trash" ? "Deleted" : "Moved to Trash");
                }}
              >
                <Trash2 size={14} />
              </Button>
            </div>
            <Show when={d().m.folder === "requests"}>
              <Notice class="mx-3 mt-2">
                First message from this sender. Accept moves it to the Inbox and files their next messages there; Block drops everything from them. Trust score {d().m.trust_score}.
              </Notice>
            </Show>
            <Show when={d().m.folder === "spam"}>
              <Notice class="mx-3 mt-2" strong>
                Filed as spam{!d().m.sender_matches ? ": the From line claims a different identity than the one that sent it" : ""}. Links are shown as text.
              </Notice>
            </Show>
            <div class="min-h-0 flex-1 overflow-auto">
              <For each={messages()}>
                {(m) => (
                  <Message
                    m={m}
                    liveVersions={liveVersions() ?? {}}
                    onReply={(all) => props.onReply(m.id, all)}
                    onForward={() => props.onForward(m.id)}
                    expanded={m.id === d().m.id}
                  />
                )}
              </For>
            </div>
            <div class="flex shrink-0 items-center gap-2 border-t border-border px-3 py-2">
              <Button size="sm" variant="secondary" onClick={() => props.onReply(d().m.id, false)}>
                <Reply size={12} /> {t("mail_reply")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => props.onReply(d().m.id, true)}>
                <ReplyAll size={12} /> {t("mail_reply_all")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => props.onForward(d().m.id)}>
                <Forward size={12} /> {t("mail_forward")}
              </Button>
              <span class="flex-1" />
              <button type="button" class="text-[11px] text-muted hover:text-fg" onClick={() => void refetch()}>
                refresh
              </button>
            </div>
          </div>
        )}
      </Show>
    </Show>
  );
}
