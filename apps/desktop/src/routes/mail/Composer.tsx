// The composer. Recipients become chips resolved on blur through the chain
// (address, @name, name@hashgram.io, or an external e-mail when a gateway
// is configured). Attachments are staged on the Rust side under the draft
// id; the composer only sees names and sizes. Ctrl+Enter sends.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { X, Paperclip, HardDrive, Send, Trash2, Minus, ShieldAlert, ShieldCheck, Globe } from "lucide-solid";
import { Button, Checkbox, Dialog, Input, Select, Textarea, Badge } from "~/components/ui";
import { ipc, errText, type DraftView, type RecipientResolution, type EntryView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatBytes, splitRecipients, handle } from "~/lib/format";
import { pickFile, confirm } from "~/lib/dialogs";

export interface ComposerOpen {
  draftId?: string;
  replyTo?: string;
  all?: boolean;
  forward?: string;
  to?: string[];
  /** Prefilled subject for a fresh draft (e.g. a wall invitation). */
  subject?: string;
  /** Prefilled body for a fresh draft. */
  body?: string;
}

const MAX_RECIPIENTS = 100;
const MAX_SUBJECT = 998;
const MAX_BODY = 1024 * 1024;

type Chip = { input: string; res?: RecipientResolution; pending: boolean };

function RecipientField(props: { label: string; chips: Chip[]; onChange: (c: Chip[]) => void; autofocus?: boolean }) {
  const [text, setText] = createSignal("");
  let input!: HTMLInputElement;
  const resolve = async (chips: Chip[]) => {
    const need = chips.filter((c) => c.pending).map((c) => c.input);
    if (!need.length) return;
    try {
      const res = await ipc.mailResolveRecipients(need);
      const byInput = new Map(res.map((r) => [r.input, r]));
      props.onChange(chips.map((c) => (c.pending ? { input: c.input, res: byInput.get(c.input.trim()), pending: false } : c)));
    } catch (e) {
      props.onChange(chips.map((c) => (c.pending ? { input: c.input, res: { input: c.input, kind: "invalid", address: "", username: "", display_name: "", has_identity: false, devices: 0, error: errText(e) }, pending: false } : c)));
    }
  };
  const commit = () => {
    const parts = splitRecipients(text());
    if (!parts.length) return;
    setText("");
    const next: Chip[] = [...props.chips];
    for (const p of parts) if (!next.some((c) => c.input.toLowerCase() === p.toLowerCase())) next.push({ input: p, pending: true });
    props.onChange(next.slice(0, MAX_RECIPIENTS));
    void resolve(next);
  };
  onMount(() => {
    if (props.autofocus) queueMicrotask(() => input?.focus());
    // Chips that arrived pre-filled (reply) still need resolving.
    if (props.chips.some((c) => c.pending)) void resolve(props.chips);
  });
  const chipCls = (c: Chip) => {
    if (c.pending) return "border-border text-muted";
    if (!c.res || c.res.kind === "invalid" || c.res.error) return "border-fg text-fg";
    if (c.res.kind === "external") return "border-accent text-muted";
    return "border-border text-fg";
  };
  return (
    <div class="flex items-start gap-2 border-b border-border py-1.5">
      <span class="mt-1 w-12 shrink-0 text-xs text-muted">{props.label}</span>
      <div class="flex min-w-0 flex-1 flex-wrap items-center gap-1" onClick={() => input?.focus()}>
        <For each={props.chips}>
          {(c) => (
            <span
              class={`inline-flex h-6 max-w-full items-center gap-1 rounded-full border px-2 text-xs ${chipCls(c)}`}
              title={c.res?.error || (c.res ? `${c.res.address}${c.res.devices ? ` · ${c.res.devices} device(s)` : ""}` : "resolving…")}
              data-chip={c.input}
              data-kind={c.res?.kind ?? "pending"}
            >
              <Show when={c.res?.kind === "hashgram" && !c.res?.error}>
                <ShieldCheck size={10} class="text-brand" />
              </Show>
              <Show when={c.res?.kind === "external"}>
                <Globe size={10} />
              </Show>
              <Show when={c.res && (c.res.kind === "invalid" || c.res.error)}>
                <ShieldAlert size={10} />
              </Show>
              <span class="truncate">{c.res && c.res.kind === "hashgram" ? handle(c.res.address, c.res.username, c.res.display_name) : c.input}</span>
              <button type="button" class="text-muted hover:text-fg" aria-label={`remove ${c.input}`} onClick={() => props.onChange(props.chips.filter((x) => x !== c))}>
                <X size={10} />
              </button>
            </span>
          )}
        </For>
        <input
          ref={input}
          class="h-6 min-w-[8rem] flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted"
          value={text()}
          placeholder={props.chips.length ? "" : "@name, name@hashgram.io or hash1…"}
          onInput={(e) => setText(e.currentTarget.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === "," || e.key === ";" || e.key === "Tab") {
              if (text().trim()) {
                e.preventDefault();
                commit();
              }
            } else if (e.key === "Backspace" && !text() && props.chips.length) {
              props.onChange(props.chips.slice(0, -1));
            }
          }}
          autocomplete="off"
          spellcheck={false}
        />
      </div>
    </div>
  );
}

function DrivePicker(props: { open: boolean; onClose: () => void; onPick: (e: EntryView, live: boolean) => void }) {
  const [parent, setParent] = createSignal("");
  const [crumbs, setCrumbs] = createSignal<{ id: string; name: string }[]>([]);
  const [live, setLive] = createSignal(false);
  const [entries] = createResource(
    () => (props.open ? parent() : null),
    (p) => ipc.driveList(p),
  );
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("mail_attach_drive")} width="max-w-md">
      <div class="mb-2 flex items-center gap-1 text-xs text-muted">
        <button type="button" class="hover:text-fg" onClick={() => { setParent(""); setCrumbs([]); }}>
          {t("drive_my")}
        </button>
        <For each={crumbs()}>
          {(c, i) => (
            <>
              <span>/</span>
              <button type="button" class="hover:text-fg" onClick={() => { setParent(c.id); setCrumbs(crumbs().slice(0, i() + 1)); }}>
                {c.name}
              </button>
            </>
          )}
        </For>
      </div>
      <div class="card max-h-72 overflow-auto">
        <For each={entries() ?? []} fallback={<div class="p-4 text-center text-xs text-muted">{entries.loading ? t("loading") : "empty"}</div>}>
          {(e) => (
            <button
              type="button"
              class="row flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px]"
              onClick={() => {
                if (e.kind === "folder") {
                  setParent(e.id);
                  setCrumbs([...crumbs(), { id: e.id, name: e.name }]);
                } else props.onPick(e, live());
              }}
            >
              <span class="flex-1 truncate">{e.name}</span>
              <span class="tnum text-xs text-muted">{e.kind === "file" ? formatBytes(e.size) : "folder"}</span>
            </button>
          )}
        </For>
      </div>
      <div class="mt-3 flex flex-col gap-1 text-xs">
        <label class="flex items-start gap-2">
          <input type="radio" name="mode" checked={!live()} onChange={() => setLive(false)} />
          <span>
            <b>{t("drive_snapshot")}</b> — {t("drive_snapshot_hint")}
          </span>
        </label>
        <label class="flex items-start gap-2">
          <input type="radio" name="mode" checked={live()} onChange={() => setLive(true)} />
          <span>
            <b>{t("drive_live")}</b> — {t("drive_live_hint")}
          </span>
        </label>
      </div>
    </Dialog>
  );
}

export function Composer(props: { open: ComposerOpen | null; onClose: () => void; onSent: () => void }) {
  const [draft, setDraft] = createSignal<DraftView | null>(null);
  const [to, setTo] = createSignal<Chip[]>([]);
  const [cc, setCc] = createSignal<Chip[]>([]);
  const [bcc, setBcc] = createSignal<Chip[]>([]);
  const [showCc, setShowCc] = createSignal(false);
  const [showBcc, setShowBcc] = createSignal(false);
  const [subject, setSubject] = createSignal("");
  const [body, setBody] = createSignal("");
  const [receipt, setReceipt] = createSignal(false);
  const [importance, setImportance] = createSignal("0");
  const [busy, setBusy] = createSignal(false);
  const [sending, setSending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [drivePick, setDrivePick] = createSignal(false);
  const [minimized, setMinimized] = createSignal(false);
  const gateway = () => !!store.settings()?.network.gateway_address.trim();
  let bodyRef!: HTMLTextAreaElement;
  let dirty = false;
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  const fields = () => ({
    to: to().map((c) => c.input),
    cc: cc().map((c) => c.input),
    bcc: bcc().map((c) => c.input),
    subject: subject(),
    body_text: body(),
    body_html: "",
  });

  const load = async (o: ComposerOpen) => {
    setError(null);
    try {
      const d = o.draftId ? await ipc.mailDraftGet(o.draftId) : await ipc.mailDraftNew({ replyTo: o.replyTo, all: o.all, forward: o.forward });
      setDraft(d);
      setTo([...(o.to ?? []), ...d.to].map((x) => ({ input: x, pending: true })));
      setCc(d.cc.map((x) => ({ input: x, pending: true })));
      setBcc(d.bcc.map((x) => ({ input: x, pending: true })));
      setShowCc(d.cc.length > 0);
      setShowBcc(d.bcc.length > 0);
      setSubject(d.subject || o.subject || "");
      setBody(d.body_text || o.body || "");
      dirty = !!(o.subject || o.body) && !d.subject && !d.body_text;
      if (dirty) scheduleSave();
      queueMicrotask(() => {
        if (o.replyTo || o.to?.length) bodyRef?.focus();
      });
    } catch (e) {
      setError(errText(e));
    }
  };
  createEffect(() => {
    const o = props.open;
    if (o) void load(o);
    else setDraft(null);
  });

  const scheduleSave = () => {
    dirty = true;
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => void saveNow(), 1500);
  };
  const saveNow = async () => {
    const d = draft();
    if (!d || !dirty) return;
    dirty = false;
    try {
      const nd = await ipc.mailDraftSave(d.id, fields());
      setDraft((cur) => (cur ? { ...cur, updated_at_ms: nd.updated_at_ms } : nd));
    } catch {
      dirty = true;
    }
  };
  onCleanup(() => {
    if (saveTimer) clearTimeout(saveTimer);
  });

  const attachFile = async () => {
    const d = draft();
    if (!d) return;
    const files = await pickFile({ multiple: true, title: t("mail_attach") });
    if (!files.length) return;
    setBusy(true);
    setError(null);
    try {
      let nd = d;
      for (const f of files) nd = await ipc.mailAttachFile(d.id, f);
      setDraft(nd);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const attachDrive = async (e: EntryView, live: boolean) => {
    const d = draft();
    if (!d) return;
    setDrivePick(false);
    await saveNow();
    setBusy(true);
    setError(null);
    try {
      setDraft(await ipc.mailAttachDrive(d.id, e.id, live));
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  };
  const removeAttachment = async (index: number) => {
    const d = draft();
    if (!d) return;
    try {
      setDraft(await ipc.mailDraftRemoveAttachment(d.id, index));
    } catch (e) {
      setError(errText(e));
    }
  };
  const onDrop = async (e: DragEvent) => {
    e.preventDefault();
    const d = draft();
    const files = e.dataTransfer?.files;
    if (!d || !files?.length) return;
    setBusy(true);
    try {
      let nd = d;
      for (const f of Array.from(files)) {
        if (f.size > 8 * 1024 * 1024) {
          store.toast(`${f.name}: use Attach file for files over 8 MiB`, "error");
          continue;
        }
        const buf = new Uint8Array(await f.arrayBuffer());
        let bin = "";
        for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
        nd = await ipc.mailAttachBytes(d.id, f.name, f.type, btoa(bin));
      }
      setDraft(nd);
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  };

  const problems = createMemo(() => {
    const all = [...to(), ...cc(), ...bcc()];
    const out: string[] = [];
    if (store.identity()?.this_device_registered === false) {
      out.push("finish identity setup in Wallet → Devices before sending mail");
    }
    if (!all.length) out.push("add at least one recipient");
    if (all.some((c) => c.pending)) out.push("resolving recipients…");
    for (const c of all) if (c.res?.error) out.push(`${c.input}: ${c.res.error}`);
    if (all.length > MAX_RECIPIENTS) out.push(`at most ${MAX_RECIPIENTS} recipients`);
    if (subject().length > MAX_SUBJECT) out.push("subject too long");
    if (body().length > MAX_BODY) out.push("body over 1 MiB");
    if (all.some((c) => c.res?.kind === "external") && !gateway()) out.push("external e-mail needs a gateway (Settings → Network)");
    return out;
  });

  const send = async () => {
    const d = draft();
    if (!d || problems().length) return;
    if (!subject().trim() && !(await confirm("Send without a subject?"))) return;
    setSending(true);
    setError(null);
    try {
      await ipc.mailSend(d.id, fields(), { request_read_receipt: receipt(), importance: Number(importance()) });
      store.toast("Sent");
      store.bump("mail");
      props.onSent();
    } catch (e) {
      setError(errText(e));
    } finally {
      setSending(false);
    }
  };
  const discard = async () => {
    const d = draft();
    if (d && (body().trim() || subject().trim() || d.attachments.length) && !(await confirm("Discard this draft?"))) return;
    if (d) await ipc.mailDraftDelete(d.id).catch(() => undefined);
    store.bump("mail");
    props.onClose();
  };
  const closeKeep = async () => {
    await saveNow();
    store.bump("mail");
    props.onClose();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.ctrlKey && e.key === "Enter") {
      e.preventDefault();
      void send();
    } else if (e.key === "Escape") {
      e.stopPropagation();
      void closeKeep();
    }
  };

  return (
    <Show when={props.open && draft()}>
      <div
        class={`card fixed bottom-3 right-3 z-40 flex w-[640px] max-w-[calc(100vw-2rem)] flex-col fade-in ${minimized() ? "h-10" : "max-h-[85vh]"}`}
        role="dialog"
        aria-label={t("mail_compose")}
        onKeyDown={onKey}
        onDragOver={(e) => e.preventDefault()}
        onDrop={(e) => void onDrop(e)}
        data-testid="composer"
      >
        <div class="flex h-10 shrink-0 items-center gap-2 border-b border-border px-3">
          <span class="min-w-0 flex-1 truncate text-[13px] font-semibold">{subject() || t("mail_compose")}</span>
          <Button variant="ghost" size="icon-sm" title={minimized() ? "Expand" : "Minimize"} onClick={() => setMinimized((v) => !v)}>
            <Minus size={14} />
          </Button>
          <Button variant="ghost" size="icon-sm" title="Close (keeps the draft)" onClick={() => void closeKeep()}>
            <X size={14} />
          </Button>
        </div>
        <Show when={!minimized()}>
          <div class="min-h-0 flex-1 overflow-auto px-3">
            <RecipientField label={t("mail_to")} chips={to()} onChange={(c) => { setTo(c); scheduleSave(); }} autofocus={!props.open?.replyTo} />
            <Show when={showCc()}>
              <RecipientField label={t("mail_cc")} chips={cc()} onChange={(c) => { setCc(c); scheduleSave(); }} />
            </Show>
            <Show when={showBcc()}>
              <RecipientField label={t("mail_bcc_field")} chips={bcc()} onChange={(c) => { setBcc(c); scheduleSave(); }} />
            </Show>
            <div class="flex items-center gap-2 border-b border-border py-1.5">
              <span class="w-12 shrink-0 text-xs text-muted">{t("mail_subject")}</span>
              <input class="h-6 flex-1 bg-transparent text-[13px] outline-none" value={subject()} maxLength={MAX_SUBJECT} onInput={(e) => { setSubject(e.currentTarget.value); scheduleSave(); }} spellcheck={true} />
              <Show when={!showCc()}>
                <button type="button" class="text-xs text-muted hover:text-fg" onClick={() => setShowCc(true)}>
                  {t("mail_cc")}
                </button>
              </Show>
              <Show when={!showBcc()}>
                <button type="button" class="text-xs text-muted hover:text-fg" onClick={() => setShowBcc(true)}>
                  {t("mail_bcc_field")}
                </button>
              </Show>
            </div>
            <Textarea
              ref={bodyRef}
              class="my-2 min-h-48 border-0 bg-transparent px-0 focus:border-0"
              value={body()}
              onInput={(e) => { setBody(e.currentTarget.value); scheduleSave(); }}
              spellcheck={true}
              placeholder="Write your message…"
            />
            <Show when={draft()?.attachments.length}>
              <div class="mb-2 flex flex-col gap-1">
                <For each={draft()?.attachments ?? []}>
                  {(a) => (
                    <div class="flex items-center gap-2 rounded-md border border-border px-2 py-1 text-xs">
                      <Paperclip size={11} class="text-muted" />
                      <span class="min-w-0 flex-1 truncate">{a.name}</span>
                      <span class="tnum text-muted">{formatBytes(a.size)}</span>
                      <Show when={a.kind === "drive"}>
                        <Badge brand={a.live}>{a.live ? t("drive_live") : t("drive_snapshot")}</Badge>
                      </Show>
                      <Show when={a.kind === "inline"}>
                        <Badge>inline</Badge>
                      </Show>
                      <button type="button" class="text-muted hover:text-fg" aria-label="remove" onClick={() => void removeAttachment(a.index)}>
                        <X size={11} />
                      </button>
                    </div>
                  )}
                </For>
              </div>
            </Show>
            <Show when={error()}>
              <p class="mb-2 text-xs text-fg" role="alert">
                {error()}
              </p>
            </Show>
            <Show when={problems().length && (to().length || cc().length || bcc().length)}>
              <p class="mb-2 text-[11px] text-muted" data-testid="composer-problems">
                {problems().join(" · ")}
              </p>
            </Show>
          </div>
          <div class="flex shrink-0 items-center gap-1.5 border-t border-border px-3 py-2">
            <Button variant="brand" size="sm" onClick={send} disabled={problems().length > 0 || busy()} loading={sending()} title="Ctrl+Enter">
              <Send size={12} /> {t("send")}
            </Button>
            <Button variant="ghost" size="sm" onClick={attachFile} disabled={busy()} title={t("mail_attach")}>
              <Paperclip size={12} />
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setDrivePick(true)} disabled={busy()} title={t("mail_attach_drive")}>
              <HardDrive size={12} />
            </Button>
            <Select value={importance()} onChange={setImportance} options={[{ value: "0", label: "Normal" }, { value: "2", label: "High" }, { value: "1", label: "Low" }]} class="w-24 text-xs" aria-label="importance" />
            <Checkbox checked={receipt()} onChange={setReceipt} label={t("mail_read_receipt")} />
            <span class="flex-1" />
            <Show when={busy()}>
              <span class="text-[11px] text-muted">attaching…</span>
            </Show>
            <Button variant="ghost" size="sm" onClick={discard} title="Discard draft">
              <Trash2 size={12} />
            </Button>
          </div>
        </Show>
        <DrivePicker open={drivePick()} onClose={() => setDrivePick(false)} onPick={(e, live) => void attachDrive(e, live)} />
      </div>
    </Show>
  );
}
