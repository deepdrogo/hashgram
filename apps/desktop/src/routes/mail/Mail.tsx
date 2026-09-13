// Mail: three panes — folders, list, reading pane — plus the composer.
// Home screen. Keyboard: c r a f e # s j k / and Ctrl+Enter in the composer.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { Inbox, Star, Send, FileText, Archive, ShieldAlert, Trash2, Tag, PenSquare, Search, UserPlus, Rows3, List, RefreshCw } from "lucide-solid";
import { Button, Input, Menu, useContextMenu, Kbd } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { ipc, errText, type MailSummary, type DraftView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t, type Key } from "~/lib/i18n";
import { shortWhen } from "~/lib/format";
import { MailList } from "./MailList";
import { Reader } from "./Reader";
import { Composer, type ComposerOpen } from "./Composer";
import { confirm } from "~/lib/dialogs";

const FOLDERS: { id: string; key: Key; icon: typeof Inbox }[] = [
  { id: "inbox", key: "mail_inbox", icon: Inbox },
  { id: "requests", key: "mail_requests", icon: UserPlus },
  { id: "starred", key: "mail_starred", icon: Star },
  { id: "sent", key: "mail_sent", icon: Send },
  { id: "drafts", key: "mail_drafts", icon: FileText },
  { id: "archive", key: "mail_archive", icon: Archive },
  { id: "spam", key: "mail_spam", icon: ShieldAlert },
  { id: "trash", key: "mail_trash", icon: Trash2 },
];

export function MailRoute() {
  const params = useParams<{ folder?: string; id?: string }>();
  const [search, setSearch] = useSearchParams<{ compose?: string; q?: string; label?: string; subject?: string; body?: string }>();
  const navigate = useNavigate();
  const folder = () => params.folder || "inbox";
  const selected = () => params.id ?? null;
  const [query, setQuery] = createSignal("");
  const [threaded, setThreaded] = createSignal(store.settings()?.mail.threaded ?? true);
  const [composer, setComposer] = createSignal<ComposerOpen | null>(null);
  const [error, setError] = createSignal<unknown>(null);
  const menu = useContextMenu();
  const [menuFor, setMenuFor] = createSignal<MailSummary | null>(null);
  let searchRef!: HTMLInputElement;

  createEffect(() => {
    if (search.compose) {
      setComposer({ to: search.q ? [search.q] : undefined, subject: search.subject || undefined, body: search.body || undefined });
      setSearch({ compose: undefined, q: undefined, subject: undefined, body: undefined });
    }
  });

  const listKey = () => ({ folder: folder(), q: query().trim(), threaded: threaded(), tick: store.ticks().mail, label: search.label });
  const [items, { refetch }] = createResource(listKey, async (k) => {
    setError(null);
    try {
      if (k.folder === "drafts") return [] as MailSummary[];
      if (k.q.length >= 2) return await ipc.mailSearch(k.q, 200);
      const f = k.label ? `label:${k.label}` : k.folder;
      return await ipc.mailList(f, { limit: 300, threaded: k.threaded && f !== "starred" && !k.label });
    } catch (e) {
      setError(e);
      return [] as MailSummary[];
    }
  });
  const [drafts, { refetch: refetchDrafts }] = createResource(
    () => ({ f: folder(), tick: store.ticks().mail }),
    (k) => (k.f === "drafts" ? ipc.mailDraftList().catch(() => [] as DraftView[]) : Promise.resolve([] as DraftView[])),
  );
  const labels = createMemo(() => {
    const s = new Set<string>();
    for (const m of items() ?? []) for (const l of m.labels) s.add(l);
    return [...s].sort();
  });

  const select = (m: MailSummary) => navigate(`/mail/${folder()}/${m.id}`);
  const idx = () => (items() ?? []).findIndex((m) => m.id === selected());
  const move = (d: number) => {
    const list = items() ?? [];
    if (!list.length) return;
    const n = Math.max(0, Math.min(list.length - 1, (idx() < 0 ? -1 : idx()) + d));
    const m = list[n];
    if (m) select(m);
  };
  const withSel = async (f: (id: string) => Promise<unknown>, done?: string) => {
    const id = selected();
    if (!id) return;
    try {
      await f(id);
      if (done) store.toast(done);
      store.bump("mail");
      void store.refreshCounts();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable], [role=dialog]")) return;
      if (e.ctrlKey || e.altKey || e.metaKey) return;
      switch (e.key) {
        case "c":
          e.preventDefault();
          setComposer({});
          break;
        case "r":
          if (selected()) setComposer({ replyTo: selected()!, all: false });
          break;
        case "a":
          if (selected()) setComposer({ replyTo: selected()!, all: true });
          break;
        case "f":
          if (selected()) setComposer({ forward: selected()! });
          break;
        case "e":
          void withSel((id) => ipc.mailArchive(id), "Archived");
          break;
        case "#":
          void withSel((id) => ipc.mailTrash(id), "Moved to Trash");
          break;
        case "s":
          void withSel(async (id) => {
            const m = (items() ?? []).find((x) => x.id === id);
            await ipc.mailStar(id, !(m?.starred ?? false));
          });
          break;
        case "j":
        case "ArrowDown":
          e.preventDefault();
          move(1);
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          move(-1);
          break;
        case "/":
          e.preventDefault();
          searchRef?.focus();
          break;
        default:
          return;
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const count = (id: string) => store.counts()[id];
  const emptyFor = (): { title: string; hint?: string } => {
    if (query().trim().length >= 2) return { title: "No matches", hint: "Search covers subjects, bodies, senders and attachment names on this device." };
    switch (folder()) {
      case "inbox":
        return { title: t("mail_empty_inbox"), hint: "Mail from people you know lands here. Mail from strangers goes to Requests first." };
      case "requests":
        return { title: t("mail_empty_requests"), hint: "First messages from people you have never written to appear here." };
      case "sent":
        return { title: "Nothing sent yet", hint: "Press c to compose." };
      default:
        return { title: t("nothing_here") };
    }
  };

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="flex min-h-0 flex-1">
        <aside class="pane w-[188px] shrink-0">
          <div class="p-2">
            <Button variant="brand" class="w-full" onClick={() => setComposer({})} title="c">
              <PenSquare size={14} /> {t("mail_compose")}
            </Button>
          </div>
          <ul class="space-y-px px-2">
            <For each={FOLDERS}>
              {(f) => {
                const active = () => folder() === f.id && !search.label;
                const n = () => (f.id === "requests" ? count("requests")?.total : f.id === "drafts" ? drafts()?.length : count(f.id)?.unread) ?? 0;
                return (
                  <li>
                    <button
                      type="button"
                      class={`row flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] ${active() ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`}
                      aria-current={active() ? "page" : undefined}
                      onClick={() => {
                        setQuery("");
                        setSearch({ label: undefined });
                        navigate(`/mail/${f.id}`);
                      }}
                      data-folder={f.id}
                    >
                      <f.icon size={13} class={active() ? "text-brand" : ""} />
                      <span class="flex-1 text-left">{t(f.key)}</span>
                      <Show when={n() > 0}>
                        <span class={`tnum text-[11px] ${f.id === "requests" ? "badge-strong" : "text-muted"}`}>{n()}</span>
                      </Show>
                    </button>
                  </li>
                );
              }}
            </For>
          </ul>
          <Show when={labels().length}>
            <p class="px-3 pt-3 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted">{t("mail_labels")}</p>
            <ul class="space-y-px px-2">
              <For each={labels()}>
                {(l) => (
                  <li>
                    <button
                      type="button"
                      class={`row flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] ${search.label === l ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`}
                      onClick={() => setSearch({ label: l })}
                    >
                      <Tag size={13} />
                      <span class="flex-1 truncate text-left">{l}</span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <div class="mt-auto px-3 py-2 text-[11px] text-muted">
            <Kbd>c</Kbd> compose · <Kbd>j</Kbd>/<Kbd>k</Kbd> move · <Kbd>/</Kbd> search
          </div>
        </aside>
        <section class="pane w-[340px] shrink-0">
          <div class="pane-head">
            <Search size={13} class="text-muted" />
            <Input ref={searchRef} class="h-7 border-0 bg-transparent px-0" placeholder={`${t("search")}…`} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} onKeyDown={(e) => e.key === "Escape" && (setQuery(""), e.currentTarget.blur())} />
            <Button variant="ghost" size="icon-sm" title={threaded() ? "Show every message" : "Group by conversation"} onClick={() => setThreaded((v) => !v)} aria-pressed={threaded()}>
              <Show when={threaded()} fallback={<List size={14} />}>
                <Rows3 size={14} />
              </Show>
            </Button>
            <Button variant="ghost" size="icon-sm" title={t("sync_now")} onClick={() => { void ipc.syncNow().catch(() => undefined); void refetch(); void refetchDrafts(); }}>
              <RefreshCw size={13} />
            </Button>
          </div>
          <div class="min-h-0 flex-1">
            <Show when={!error()} fallback={<ErrorState error={error()} onRetry={() => void refetch()} compact />}>
              <Show
                when={folder() !== "drafts"}
                fallback={
                  <DraftList
                    drafts={drafts() ?? []}
                    onOpen={(d) => setComposer({ draftId: d.id })}
                    onDelete={async (d) => {
                      if (await confirm("Delete this draft?")) {
                        await ipc.mailDraftDelete(d.id).catch(() => undefined);
                        void refetchDrafts();
                      }
                    }}
                  />
                }
              >
                <MailList
                  items={items() ?? []}
                  selected={selected()}
                  onSelect={select}
                  emptyTitle={emptyFor().title}
                  emptyHint={emptyFor().hint}
                  onContext={(m, e) => {
                    setMenuFor(m);
                    menu.open(e);
                  }}
                />
              </Show>
            </Show>
          </div>
        </section>
        <section class="min-w-0 flex-1 bg-bg">
          <Reader
            id={folder() === "drafts" ? null : selected()}
            folder={folder()}
            threaded={threaded()}
            onReply={(id, all) => setComposer({ replyTo: id, all })}
            onForward={(id) => setComposer({ forward: id })}
            onChanged={() => {
              store.bump("mail");
            }}
          />
        </section>
      </div>
      <Menu
        open={menu.state().open}
        x={menu.state().x}
        y={menu.state().y}
        onClose={menu.close}
        items={(() => {
          const m = menuFor();
          if (!m) return [];
          const run = (f: () => Promise<unknown>, done?: string) => () =>
            void f()
              .then(() => {
                if (done) store.toast(done);
                store.bump("mail");
                void store.refreshCounts();
              })
              .catch((e) => store.toast(errText(e), "error"));
          return [
            { label: t("mail_reply"), onSelect: () => setComposer({ replyTo: m.id, all: false }) },
            { label: t("mail_reply_all"), onSelect: () => setComposer({ replyTo: m.id, all: true }) },
            { label: t("mail_forward"), onSelect: () => setComposer({ forward: m.id }) },
            { separator: true, label: "" },
            { label: m.read ? t("mail_mark_unread") : "Mark read", onSelect: run(() => ipc.mailMarkRead(m.id, !m.read)) },
            { label: m.starred ? t("mail_unstar") : t("mail_star"), onSelect: run(() => ipc.mailStar(m.id, !m.starred)) },
            ...(m.folder === "requests"
              ? [
                  { label: t("mail_accept"), onSelect: run(() => ipc.mailAcceptRequest(m.id), "Moved to Inbox") },
                  {
                    label: t("mail_block"),
                    danger: true,
                    onSelect: run(async () => {
                      await ipc.peopleBlock(m.from);
                      await ipc.mailTrash(m.id);
                    }, "Sender blocked"),
                  },
                ]
              : []),
            { separator: true, label: "" },
            { label: t("mail_archive_action"), onSelect: run(() => ipc.mailArchive(m.id), "Archived"), disabled: m.folder === "archive" },
            { label: "Move to Inbox", onSelect: run(() => ipc.mailMove(m.id, "inbox")), disabled: m.folder === "inbox" },
            { label: "Mark as spam", onSelect: run(() => ipc.mailMove(m.id, "spam")), disabled: m.folder === "spam" },
            { label: m.folder === "trash" ? "Delete permanently" : t("mail_trash_action"), danger: true, onSelect: run(() => ipc.mailTrash(m.id)) },
          ];
        })()}
      />
      <Composer
        open={composer()}
        onClose={() => setComposer(null)}
        onSent={() => {
          setComposer(null);
          navigate("/mail/sent");
        }}
      />
    </div>
  );
}

function DraftList(props: { drafts: DraftView[]; onOpen: (d: DraftView) => void; onDelete: (d: DraftView) => void }) {
  return (
    <Show when={props.drafts.length} fallback={<div class="p-6 text-center text-xs text-muted">No drafts. Drafts save themselves as you type.</div>}>
      <ul>
        <For each={props.drafts}>
          {(d) => (
            <li class="row flex cursor-default flex-col gap-0.5 border-b border-border px-3 py-2" onClick={() => props.onOpen(d)}>
              <div class="flex items-center gap-2">
                <span class="min-w-0 flex-1 truncate text-[13px]">{d.to.length ? `To ${d.to.join(", ")}` : <span class="text-muted">no recipient</span>}</span>
                <span class="tnum text-[11px] text-muted">{shortWhen(d.updated_at_ms)}</span>
                <button
                  type="button"
                  class="text-muted hover:text-fg"
                  aria-label="delete draft"
                  onClick={(e) => {
                    e.stopPropagation();
                    props.onDelete(d);
                  }}
                >
                  <Trash2 size={12} />
                </button>
              </div>
              <div class="truncate text-[13px] font-medium">{d.subject || "(no subject)"}</div>
              <div class="truncate text-xs text-muted">{d.body_text.slice(0, 120).replace(/\s+/g, " ")}</div>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}
