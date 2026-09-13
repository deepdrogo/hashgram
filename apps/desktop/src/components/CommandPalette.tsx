// Ctrl+K: one box that searches Mail, Drive and People together, plus a
// few commands (compose, sections). Results come from local indexes; a
// typed @name or address is resolved on chain only when Enter is pressed
// on the "look up" row.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { useNavigate } from "@solidjs/router";
import { Mail, HardDrive, Users, Search, ArrowRight, PenSquare } from "lucide-solid";
import { ipc, type MailSummary, type EntryView, type ContactRecord } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { handle, shortWhen, formatBytes, isHashAddress } from "~/lib/format";
import { NAV } from "./Shell";
import { Kbd } from "./ui";

type Row =
  | { kind: "cmd"; id: string; label: string; hint?: string; run: () => void }
  | { kind: "mail"; id: string; m: MailSummary }
  | { kind: "drive"; id: string; e: EntryView }
  | { kind: "person"; id: string; c: ContactRecord }
  | { kind: "lookup"; id: string; q: string };

export function CommandPalette(props: { open: boolean; onClose: () => void }) {
  const navigate = useNavigate();
  const [q, setQ] = createSignal("");
  const [sel, setSel] = createSignal(0);
  let input!: HTMLInputElement;

  createEffect(() => {
    if (props.open) {
      setQ("");
      setSel(0);
      queueMicrotask(() => input?.focus());
    }
  });

  const [results] = createResource(
    () => (props.open ? q().trim() : null),
    async (query) => {
      if (!query || query.length < 2 || store.locked()) return { mail: [] as MailSummary[], drive: [] as EntryView[], people: [] as ContactRecord[] };
      const [mail, drive, people] = await Promise.all([
        ipc.mailSearch(query, 8).catch(() => []),
        ipc.driveSearch(query, 8).catch(() => []),
        ipc.peopleSearchLocal(query).catch(() => []),
      ]);
      return { mail, drive, people: people.slice(0, 8) };
    },
  );

  const rows = createMemo<Row[]>(() => {
    const query = q().trim();
    const out: Row[] = [];
    const lower = query.toLowerCase();
    if (!query) {
      out.push({ kind: "cmd", id: "compose", label: t("mail_compose"), hint: "c", run: () => navigate("/mail?compose=1") });
      for (const n of NAV) out.push({ kind: "cmd", id: n.to, label: t(n.key), hint: n.accel ? `Alt+${n.accel}` : undefined, run: () => navigate(n.to) });
      return out;
    }
    for (const n of NAV) if (t(n.key).toLowerCase().includes(lower)) out.push({ kind: "cmd", id: n.to, label: t(n.key), run: () => navigate(n.to) });
    if (t("mail_compose").toLowerCase().includes(lower)) out.push({ kind: "cmd", id: "compose", label: t("mail_compose"), run: () => navigate("/mail?compose=1") });
    const r = results();
    for (const m of r?.mail ?? []) out.push({ kind: "mail", id: m.id, m });
    for (const e of r?.drive ?? []) out.push({ kind: "drive", id: e.id, e });
    for (const c of r?.people ?? []) out.push({ kind: "person", id: c.address, c });
    if (query.startsWith("@") || isHashAddress(query) || /^[a-z0-9._-]{2,32}(@hashgram\.io)?$/i.test(query)) out.push({ kind: "lookup", id: "lookup", q: query });
    return out;
  });

  const run = (row: Row) => {
    props.onClose();
    if (q().trim()) void ipc.searchNote(q().trim()).catch(() => undefined);
    switch (row.kind) {
      case "cmd":
        row.run();
        break;
      case "mail":
        navigate(`/mail/${row.m.folder}/${row.m.id}`);
        break;
      case "drive":
        navigate(`/drive/${row.e.parent_id}?select=${row.e.id}`);
        break;
      case "person":
        navigate(`/people/${row.c.address}`);
        break;
      case "lookup":
        navigate(`/people?q=${encodeURIComponent(row.q)}`);
        break;
    }
  };

  createEffect(() => {
    if (!props.open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
      else if (e.key === "ArrowDown") {
        e.preventDefault();
        setSel((s) => Math.min(rows().length - 1, s + 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSel((s) => Math.max(0, s - 1));
      } else if (e.key === "Enter") {
        const r = rows()[sel()];
        if (r) run(r);
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  createEffect(() => {
    void rows();
    setSel(0);
  });

  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-start justify-center bg-bg/70 pt-[12vh]" onClick={props.onClose} role="presentation">
          <div class="card w-full max-w-xl fade-in" role="dialog" aria-label={t("search")} onClick={(e) => e.stopPropagation()}>
            <div class="flex items-center gap-2 border-b border-border px-3">
              <Search size={14} class="text-muted" />
              <input
                ref={input}
                class="h-10 w-full bg-transparent text-sm outline-none placeholder:text-muted"
                placeholder={t("search_placeholder")}
                value={q()}
                onInput={(e) => setQ(e.currentTarget.value)}
                autocomplete="off"
                spellcheck={false}
              />
              <Kbd>Esc</Kbd>
            </div>
            <ul class="max-h-[50vh] overflow-auto py-1" role="listbox">
              <For each={rows()}>
                {(row, i) => (
                  <li
                    role="option"
                    aria-selected={sel() === i()}
                    class={`row flex cursor-default items-center gap-3 px-3 py-1.5 text-[13px] ${sel() === i() ? "bg-surface-2" : ""}`}
                    onMouseEnter={() => setSel(i())}
                    onClick={() => run(row)}
                  >
                    <Show when={row.kind === "cmd"}>
                      {(_) => {
                        const r = row as Extract<Row, { kind: "cmd" }>;
                        return (
                          <>
                            <ArrowRight size={13} class="text-muted" />
                            <span class="flex-1">{r.label}</span>
                            <Show when={r.hint}>
                              <Kbd>{r.hint}</Kbd>
                            </Show>
                          </>
                        );
                      }}
                    </Show>
                    <Show when={row.kind === "mail"}>
                      {(_) => {
                        const r = row as Extract<Row, { kind: "mail" }>;
                        return (
                          <>
                            <Mail size={13} class="text-muted" />
                            <span class="min-w-0 flex-1 truncate">
                              <span class="font-medium">{r.m.subject || "(no subject)"}</span>
                              <span class="ml-2 text-muted">{handle(r.m.from, r.m.from_username)}</span>
                            </span>
                            <span class="tnum text-xs text-muted">{shortWhen(r.m.received_at_ms)}</span>
                          </>
                        );
                      }}
                    </Show>
                    <Show when={row.kind === "drive"}>
                      {(_) => {
                        const r = row as Extract<Row, { kind: "drive" }>;
                        return (
                          <>
                            <HardDrive size={13} class="text-muted" />
                            <span class="min-w-0 flex-1 truncate">
                              {r.e.name}
                              <span class="ml-2 text-muted">{r.e.path}</span>
                            </span>
                            <span class="tnum text-xs text-muted">{r.e.kind === "file" ? formatBytes(r.e.size) : "folder"}</span>
                          </>
                        );
                      }}
                    </Show>
                    <Show when={row.kind === "person"}>
                      {(_) => {
                        const r = row as Extract<Row, { kind: "person" }>;
                        return (
                          <>
                            <Users size={13} class="text-muted" />
                            <span class="min-w-0 flex-1 truncate">{handle(r.c.address, r.c.username, r.c.display_name)}</span>
                            <span class="mono text-xs text-muted">{r.c.username ? `@${r.c.username}` : ""}</span>
                          </>
                        );
                      }}
                    </Show>
                    <Show when={row.kind === "lookup"}>
                      {(_) => {
                        const r = row as Extract<Row, { kind: "lookup" }>;
                        return (
                          <>
                            <PenSquare size={13} class="text-muted" />
                            <span class="flex-1">
                              Look up <span class="mono">{r.q}</span> on the network
                            </span>
                          </>
                        );
                      }}
                    </Show>
                  </li>
                )}
              </For>
              <Show when={!rows().length}>
                <li class="px-3 py-6 text-center text-xs text-muted">{results.loading ? t("loading") : t("nothing_here")}</li>
              </Show>
            </ul>
          </div>
        </div>
      </Portal>
    </Show>
  );
}
