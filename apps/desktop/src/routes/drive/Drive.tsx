// Drive: sidebar (My Drive / Shared with me / Starred / Trash), breadcrumbs,
// list or grid, context menu, drag-drop upload with progress, usage bar
// with the "changes will sync" badge, Share and Versions dialogs.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { Folder, File, FileText, Image, Upload, FolderPlus, LayoutGrid, List, Star, Trash2, Share2, History, Download, ExternalLink, Pencil, MoveRight, Copy as CopyIcon, RotateCcw, KeyRound, Users, ChevronRight, RefreshCw, Search } from "lucide-solid";
import { Button, Dialog, Field, Input, Menu, useContextMenu, Badge, ProgressBar, Empty, Notice } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Who } from "~/components/identity";
import { VirtualList } from "~/components/VirtualList";
import { ipc, on, errText, type EntryView, type DriveProgress, type SharedWithMeView, type ShareRecordView, type VersionView, type FolderEntryView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatBytes, formatMs, shortWhen, splitRecipients } from "~/lib/format";
import { pickFile, pickSavePath, confirm } from "~/lib/dialogs";

type View = "mine" | "shared" | "starred" | "trash";

function iconFor(e: { kind: string; mime: string }) {
  if (e.kind === "folder") return Folder;
  if (e.mime.startsWith("image/")) return Image;
  if (e.mime.startsWith("text/") || e.mime.includes("pdf")) return FileText;
  return File;
}

export function DriveRoute() {
  const params = useParams<{ parent?: string }>();
  const [search, setSearch] = useSearchParams<{ view?: string; select?: string }>();
  const navigate = useNavigate();
  const parent = () => params.parent ?? "";
  const view = (): View => (search.view as View) || "mine";
  const [grid, setGrid] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal<string | null>(search.select ?? null);
  const [progress, setProgress] = createSignal<Record<string, DriveProgress>>({});
  const [share, setShare] = createSignal<EntryView | null>(null);
  const [versions, setVersions] = createSignal<EntryView | null>(null);
  const [rename, setRename] = createSignal<EntryView | null>(null);
  const [moveTo, setMoveTo] = createSignal<EntryView | null>(null);
  const [mkdir, setMkdir] = createSignal(false);
  const [openShared, setOpenShared] = createSignal<SharedWithMeView | null>(null);
  const [dragging, setDragging] = createSignal(false);
  const menu = useContextMenu();
  const [menuFor, setMenuFor] = createSignal<EntryView | null>(null);

  const key = () => ({ view: view(), parent: parent(), q: query().trim(), tick: store.ticks().drive });
  const [entries, { refetch }] = createResource(key, async (k) => {
    if (k.q.length >= 2) return ipc.driveSearch(k.q, 300);
    if (k.view === "starred") return ipc.driveStarred();
    if (k.view === "trash") return ipc.driveTrashList();
    return ipc.driveList(k.parent);
  });
  const [usage, { refetch: refetchUsage }] = createResource(
    () => store.ticks().drive,
    () => ipc.driveUsage(),
  );
  const [shared, { refetch: refetchShared }] = createResource(
    () => ({ v: view(), tick: store.ticks().drive }),
    (k) => (k.v === "shared" ? ipc.driveSharedWithMe() : Promise.resolve([] as SharedWithMeView[])),
  );
  const [crumbs] = createResource(
    () => parent(),
    async (p) => {
      const out: { id: string; name: string }[] = [];
      let cur = p;
      let guard = 0;
      while (cur && guard++ < 64) {
        const e = await ipc.driveEntry(cur).catch(() => null);
        if (!e) break;
        out.unshift({ id: e.id, name: e.name });
        cur = e.parent_id;
      }
      return out;
    },
  );
  const sorted = createMemo(() => {
    const list = [...(entries() ?? [])];
    list.sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "folder" ? -1 : 1));
    return list;
  });

  onMount(async () => {
    const un = await on("drive:progress", (p) => {
      // Rust only fills `message` on failure; keep the file name we recorded
      // when the upload was started so the row stays readable.
      setProgress((m) => ({ ...m, [p.op]: { ...p, message: p.stage === "failed" ? p.message : m[p.op]?.message || p.message } }));
      if (p.stage === "done" || p.stage === "failed") {
        setTimeout(() => setProgress((m) => { const n = { ...m }; delete n[p.op]; return n; }), 4000);
        if (p.stage === "done") { store.bump("drive"); }
      }
    });
    onCleanup(un);
  });

  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      store.bump("drive");
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  const upload = async (paths?: string[]) => {
    const files = paths ?? (await pickFile({ multiple: true, title: t("drive_upload") }));
    for (const p of files) {
      const op = `up-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
      setProgress((m) => ({ ...m, [op]: { op, stage: "reading", done: 0, total: 0, message: p.split(/[\\/]/).pop() ?? "" } }));
      ipc.driveUpload(parent(), p, op).catch((e) => store.toast(errText(e), "error"));
    }
  };
  const download = async (e: EntryView) => {
    const p = await pickSavePath(e.name);
    if (!p) return;
    await act(() => ipc.driveDownload(e.id, p), `Saved ${e.name}`);
  };
  const openEntry = (e: EntryView) => {
    if (e.kind === "folder") {
      setQuery("");
      navigate(`/drive/${e.id}`);
    } else void act(() => ipc.driveOpen(e.id));
  };
  const onDrop = async (ev: DragEvent) => {
    ev.preventDefault();
    setDragging(false);
    const files = ev.dataTransfer?.files;
    if (!files?.length) return;
    for (const f of Array.from(files)) {
      if (f.size > 32 * 1024 * 1024) {
        store.toast(`${f.name}: use Upload for files over 32 MiB`, "error");
        continue;
      }
      const buf = new Uint8Array(await f.arrayBuffer());
      let bin = "";
      for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
      await act(() => ipc.driveUploadBytes(parent(), f.name, f.type, btoa(bin)), `Uploaded ${f.name}`);
    }
  };

  const contextItems = (e: EntryView) => {
    const trashed = view() === "trash";
    return [
      { label: t("drive_open"), icon: <ExternalLink size={13} />, onSelect: () => openEntry(e), disabled: trashed },
      { label: t("drive_download"), icon: <Download size={13} />, onSelect: () => void download(e), disabled: e.kind === "folder" || trashed },
      { separator: true, label: "" },
      { label: t("drive_rename"), icon: <Pencil size={13} />, onSelect: () => setRename(e), disabled: trashed },
      { label: t("drive_move"), icon: <MoveRight size={13} />, onSelect: () => setMoveTo(e), disabled: trashed },
      { label: t("drive_copy"), icon: <CopyIcon size={13} />, onSelect: () => void act(() => ipc.driveCopy(e.id, e.parent_id, `Copy of ${e.name}`), "Copied"), disabled: e.kind === "folder" || trashed },
      { label: e.starred ? t("mail_unstar") : t("mail_star"), icon: <Star size={13} />, onSelect: () => void act(() => ipc.driveStar(e.id, !e.starred)) },
      { label: t("drive_share"), icon: <Share2 size={13} />, onSelect: () => setShare(e), disabled: trashed },
      { label: t("drive_versions"), icon: <History size={13} />, onSelect: () => setVersions(e), disabled: e.kind === "folder" },
      { label: t("drive_rekey"), icon: <KeyRound size={13} />, title: t("drive_rekey_hint"), onSelect: async () => { if (await confirm(`${t("drive_rekey_hint")}\n\nRekey "${e.name}"?`)) await act(() => ipc.driveRekey(e.id), "Rekeyed"); }, disabled: e.kind === "folder" || trashed },
      { separator: true, label: "" },
      ...(trashed
        ? [
            { label: t("drive_restore"), icon: <RotateCcw size={13} />, onSelect: () => void act(() => ipc.driveRestore(e.id), "Restored") },
            { label: "Delete permanently", icon: <Trash2 size={13} />, danger: true, onSelect: async () => { if (await confirm(`Delete "${e.name}" for good? The key is discarded; ciphertext on nodes expires on its own.`)) await act(() => ipc.driveDelete(e.id), "Deleted"); } },
          ]
        : [{ label: t("drive_trash"), icon: <Trash2 size={13} />, danger: true, onSelect: () => void act(() => ipc.driveTrash(e.id), "Moved to Trash") }]),
    ];
  };

  const sideItem = (v: View, label: string, icon: typeof Folder) => (
    <button
      type="button"
      class={`row flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] ${view() === v ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`}
      onClick={() => {
        setQuery("");
        setSearch({ view: v === "mine" ? undefined : v });
        if (v === "mine") navigate("/drive");
      }}
      data-view={v}
    >
      {icon({ size: 13, class: view() === v ? "text-brand" : "" })}
      <span class="flex-1 text-left">{label}</span>
    </button>
  );

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="flex min-h-0 flex-1">
        <aside class="pane w-[188px] shrink-0">
          <div class="flex gap-1.5 p-2">
            <Button variant="brand" class="flex-1" onClick={() => void upload()}>
              <Upload size={14} /> {t("drive_upload")}
            </Button>
            <Button variant="secondary" size="icon" title={t("drive_new_folder")} onClick={() => setMkdir(true)}>
              <FolderPlus size={14} />
            </Button>
          </div>
          <ul class="space-y-px px-2">
            <li>{sideItem("mine", t("drive_my"), Folder)}</li>
            <li>{sideItem("shared", t("drive_shared"), Users)}</li>
            <li>{sideItem("starred", t("drive_starred"), Star)}</li>
            <li>{sideItem("trash", t("drive_trash"), Trash2)}</li>
          </ul>
          <div class="mt-auto px-3 py-3 text-[11px] text-muted">
            <Show when={usage()}>
              {(u) => (
                <>
                  <div class="flex items-center justify-between">
                    <span>{formatBytes(u().bytes)}</span>
                    <span>
                      {u().files} files · {u().folders} folders
                    </span>
                  </div>
                  <ProgressBar value={u().bytes} max={Math.max(u().bytes, 10 * 1024 ** 3)} class="mt-1" label="usage" />
                  <div class="mt-1 flex items-center gap-2">
                    <span>rev {u().revision}</span>
                    <Show when={u().dirty}>
                      <Badge brand title="The encrypted folder tree is published at the next sync round.">
                        {t("drive_dirty")}
                      </Badge>
                    </Show>
                    <Show when={!u().dirty}>
                      <span>· synced</span>
                    </Show>
                  </div>
                </>
              )}
            </Show>
          </div>
        </aside>
        <section
          class={`relative flex min-w-0 flex-1 flex-col ${dragging() ? "outline outline-2 -outline-offset-4 outline-brand" : ""}`}
          onDragOver={(e) => {
            e.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(e) => void onDrop(e)}
        >
          <div class="flex h-10 shrink-0 items-center gap-1 border-b border-border px-3 text-[13px]">
            <Show when={view() === "mine" && !query().trim()} fallback={<span class="font-medium">{query().trim() ? `Search: ${query()}` : view() === "shared" ? t("drive_shared") : view() === "starred" ? t("drive_starred") : t("drive_trash")}</span>}>
              <button type="button" class={`hover:text-fg ${parent() ? "text-muted" : "font-medium"}`} onClick={() => navigate("/drive")}>
                {t("drive_my")}
              </button>
              <For each={crumbs() ?? []}>
                {(c, i) => (
                  <>
                    <ChevronRight size={12} class="text-muted" />
                    <button type="button" class={`hover:text-fg ${i() === (crumbs()?.length ?? 0) - 1 ? "font-medium" : "text-muted"}`} onClick={() => navigate(`/drive/${c.id}`)}>
                      {c.name}
                    </button>
                  </>
                )}
              </For>
            </Show>
            <span class="flex-1" />
            <div class="flex items-center gap-1 rounded-md border border-border bg-surface-2 px-2">
              <Search size={12} class="text-muted" />
              <input class="h-6 w-40 bg-transparent text-xs outline-none placeholder:text-muted" placeholder={`${t("search")}…`} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
            </div>
            <Button variant="ghost" size="icon-sm" title={grid() ? "List" : "Grid"} onClick={() => setGrid((v) => !v)}>
              <Show when={grid()} fallback={<LayoutGrid size={14} />}>
                <List size={14} />
              </Show>
            </Button>
            <Show when={view() === "trash" && (entries()?.length ?? 0) > 0}>
              <Button variant="danger" size="sm" onClick={async () => { if (await confirm("Empty the trash? Entries are deleted for good.")) await act(() => ipc.driveEmptyTrash(), "Trash emptied"); }}>
                Empty trash
              </Button>
            </Show>
            <Button variant="ghost" size="icon-sm" title="Refresh" onClick={() => { void refetch(); void refetchUsage(); void refetchShared(); }}>
              <RefreshCw size={13} />
            </Button>
          </div>
          <Show when={Object.keys(progress()).length}>
            <div class="flex flex-col gap-1 border-b border-border px-3 py-2">
              <For each={Object.values(progress())}>
                {(p) => (
                  <div class="flex items-center gap-2 text-xs">
                    <span class="w-48 truncate">{p.message || p.op}</span>
                    <ProgressBar value={p.stage === "done" ? 1 : p.done} max={p.stage === "done" ? 1 : Math.max(1, p.total)} class="flex-1" />
                    <span class="w-20 text-right text-muted">{p.stage === "failed" ? "failed" : p.stage === "done" ? "done" : p.stage === "uploading" && p.total ? `${formatBytes(p.total)}` : p.stage}</span>
                  </div>
                )}
              </For>
            </div>
          </Show>
          <div class="min-h-0 flex-1">
            <Show when={view() !== "shared"} fallback={<SharedWithMe items={shared() ?? []} loading={shared.loading} onOpenFolder={setOpenShared} onChanged={() => void refetchShared()} />}>
              <Show when={!entries.error} fallback={<ErrorState error={entries.error} onRetry={() => void refetch()} />}>
                <Show
                  when={sorted().length}
                  fallback={
                    <Empty title={view() === "trash" ? "Trash is empty" : query().trim() ? "No matches" : "This folder is empty"} icon={<Folder size={20} />}>
                      {view() === "mine" ? "Drop files here or press Upload. Everything is encrypted on this PC before it leaves." : ""}
                    </Empty>
                  }
                >
                  <Show
                    when={!grid()}
                    fallback={
                      <div class="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-2 overflow-auto p-3">
                        <For each={sorted()}>
                          {(e) => (
                            <GridTile e={e} selected={selected() === e.id} onSelect={() => setSelected(e.id)} onOpen={() => openEntry(e)} onContext={(ev) => { setMenuFor(e); setSelected(e.id); menu.open(ev); }} />
                          )}
                        </For>
                      </div>
                    }
                  >
                    <div class="flex h-full flex-col">
                      <div class="grid grid-cols-[1fr_140px_90px_70px] gap-2 border-b border-border px-3 py-1 text-[11px] text-muted">
                        <span>Name</span>
                        <span>Modified</span>
                        <span class="text-right">Size</span>
                        <span class="text-right">Versions</span>
                      </div>
                      <div class="min-h-0 flex-1">
                        <VirtualList items={sorted()} estimateSize={32} key={(e) => e.id}>
                          {(e) => {
                            const Icon = iconFor(e);
                            return (
                              <div
                                class="row grid cursor-default grid-cols-[1fr_140px_90px_70px] items-center gap-2 px-3 py-1.5 text-[13px]"
                                data-selected={selected() === e.id}
                                data-entry={e.id}
                                onClick={() => setSelected(e.id)}
                                onDblClick={() => openEntry(e)}
                                onContextMenu={(ev) => {
                                  setMenuFor(e);
                                  setSelected(e.id);
                                  menu.open(ev);
                                }}
                              >
                                <span class="flex min-w-0 items-center gap-2">
                                  <Icon size={14} class={e.kind === "folder" ? "text-brand" : "text-muted"} />
                                  <span class="truncate">{e.name}</span>
                                  <Show when={e.starred}>
                                    <Star size={11} class="text-brand" fill="currentColor" />
                                  </Show>
                                  <Show when={query().trim()}>
                                    <span class="truncate text-xs text-muted">{e.path}</span>
                                  </Show>
                                </span>
                                <span class="tnum text-xs text-muted">{shortWhen(e.modified_at_ms)}</span>
                                <span class="tnum text-right text-xs text-muted">{e.kind === "file" ? formatBytes(e.size) : "—"}</span>
                                <span class="tnum text-right text-xs text-muted">{e.kind === "file" ? e.versions + 1 : "—"}</span>
                              </div>
                            );
                          }}
                        </VirtualList>
                      </div>
                    </div>
                  </Show>
                </Show>
              </Show>
            </Show>
          </div>
        </section>
      </div>

      <Menu open={menu.state().open} x={menu.state().x} y={menu.state().y} onClose={menu.close} items={menuFor() ? contextItems(menuFor()!) : []} />

      <Dialog open={mkdir()} onClose={() => setMkdir(false)} title={t("drive_new_folder")} width="max-w-sm">
        <NameForm label="Folder name" initial="" submit="Create" onSubmit={async (n) => { await act(() => ipc.driveMkdir(parent(), n), "Folder created"); setMkdir(false); }} />
      </Dialog>
      <Dialog open={!!rename()} onClose={() => setRename(null)} title={t("drive_rename")} width="max-w-sm">
        <Show when={rename()}>{(e) => <NameForm label="New name" initial={e().name} submit="Rename" onSubmit={async (n) => { await act(() => ipc.driveRename(e().id, n)); setRename(null); }} />}</Show>
      </Dialog>
      <Dialog open={!!moveTo()} onClose={() => setMoveTo(null)} title={t("drive_move")} width="max-w-md">
        <Show when={moveTo()}>{(e) => <FolderPickerBody exclude={e().id} onPick={async (dest) => { await act(() => ipc.driveMove(e().id, dest), "Moved"); setMoveTo(null); }} />}</Show>
      </Dialog>
      <ShareDialog entry={share()} onClose={() => setShare(null)} />
      <VersionsDialog entry={versions()} onClose={() => setVersions(null)} />
      <SharedFolderDialog share={openShared()} onClose={() => setOpenShared(null)} />
    </div>
  );
}

function GridTile(props: { e: EntryView; selected: boolean; onSelect: () => void; onOpen: () => void; onContext: (e: MouseEvent) => void }) {
  const Icon = iconFor(props.e);
  const [thumb] = createResource(
    () => (props.e.kind === "file" && props.e.mime.startsWith("image/") && props.e.size < 4 * 1024 * 1024 ? props.e.id : null),
    (id) => ipc.drivePreview(id).then((b64) => `data:${props.e.mime};base64,${b64}`).catch(() => null),
  );
  return (
    <button
      type="button"
      class={`card flex flex-col items-center gap-1.5 p-3 text-center ${props.selected ? "border-brand" : "hover:border-accent"}`}
      onClick={props.onSelect}
      onDblClick={props.onOpen}
      onContextMenu={props.onContext}
    >
      <Show when={thumb()} fallback={<Icon size={28} class={props.e.kind === "folder" ? "text-brand" : "text-muted"} />}>
        <img src={thumb()!} alt="" class="h-16 w-full rounded object-cover" />
      </Show>
      <span class="w-full truncate text-xs">{props.e.name}</span>
      <span class="text-[10px] text-muted">{props.e.kind === "file" ? formatBytes(props.e.size) : "folder"}</span>
    </button>
  );
}

function NameForm(props: { label: string; initial: string; submit: string; onSubmit: (name: string) => Promise<void> }) {
  const [name, setName] = createSignal(props.initial);
  const [busy, setBusy] = createSignal(false);
  let ref!: HTMLInputElement;
  onMount(() => queueMicrotask(() => { ref?.focus(); ref?.select(); }));
  const go = async () => {
    if (!name().trim()) return;
    setBusy(true);
    try {
      await props.onSubmit(name().trim());
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="flex flex-col gap-3">
      <Field label={props.label}>
        <Input ref={ref} value={name()} onInput={(e) => setName(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void go()} maxLength={255} />
      </Field>
      <div class="flex justify-end">
        <Button onClick={go} disabled={!name().trim()} loading={busy()}>
          {props.submit}
        </Button>
      </div>
    </div>
  );
}

export function FolderPickerBody(props: { exclude?: string; onPick: (parentId: string) => Promise<void> | void }) {
  const [cur, setCur] = createSignal("");
  const [crumbs, setCrumbs] = createSignal<{ id: string; name: string }[]>([]);
  const [list] = createResource(cur, (p) => ipc.driveList(p));
  return (
    <div class="flex flex-col gap-2">
      <div class="flex items-center gap-1 text-xs text-muted">
        <button type="button" class="hover:text-fg" onClick={() => { setCur(""); setCrumbs([]); }}>
          {t("drive_my")}
        </button>
        <For each={crumbs()}>
          {(c, i) => (
            <>
              <span>/</span>
              <button type="button" class="hover:text-fg" onClick={() => { setCur(c.id); setCrumbs(crumbs().slice(0, i() + 1)); }}>
                {c.name}
              </button>
            </>
          )}
        </For>
      </div>
      <div class="card max-h-64 overflow-auto">
        <For each={(list() ?? []).filter((e) => e.kind === "folder" && e.id !== props.exclude)} fallback={<div class="p-3 text-center text-xs text-muted">no subfolders</div>}>
          {(e) => (
            <button type="button" class="row flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px]" onClick={() => { setCur(e.id); setCrumbs([...crumbs(), { id: e.id, name: e.name }]); }}>
              <Folder size={13} class="text-brand" /> {e.name}
            </button>
          )}
        </For>
      </div>
      <div class="flex justify-end">
        <Button onClick={() => void props.onPick(cur())}>Choose this folder</Button>
      </div>
    </div>
  );
}

function ShareDialog(props: { entry: EntryView | null; onClose: () => void }) {
  const [to, setTo] = createSignal("");
  const [live, setLive] = createSignal(false);
  const [note, setNote] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [tick, setTick] = createSignal(0);
  const [shares] = createResource(
    () => (props.entry ? { id: props.entry.id, tick: tick() } : null),
    (k) => ipc.driveShares(k.id),
  );
  createEffect(() => {
    if (props.entry) {
      setTo("");
      setLive(false);
      setNote("");
      setError(null);
    }
  });
  const go = async () => {
    const e = props.entry;
    if (!e) return;
    const grantees = splitRecipients(to());
    if (!grantees.length) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.driveShare(e.id, grantees, live(), note());
      store.toast(`Shared ${e.name}`);
      setTo("");
      setTick((n) => n + 1);
      store.bump("drive");
    } catch (err) {
      setError(errText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={!!props.entry} onClose={props.onClose} title={`${t("drive_share")} ${props.entry?.name ?? ""}`} width="max-w-md">
      <div class="flex flex-col gap-3" data-testid="share-dialog">
        <Field label="With" hint="@names, name@hashgram.io or hash1… addresses, comma-separated. They must have opened Hashgram once.">
          <Input value={to()} onInput={(e) => setTo(e.currentTarget.value)} placeholder="@alice, @bob" />
        </Field>
        <div class="flex flex-col gap-1 text-xs">
          <label class="flex items-start gap-2">
            <input type="radio" name="share-mode" checked={!live()} onChange={() => setLive(false)} />
            <span>
              <b>{t("drive_snapshot")}</b> — {t("drive_snapshot_hint")}
            </span>
          </label>
          <label class="flex items-start gap-2">
            <input type="radio" name="share-mode" checked={live()} onChange={() => setLive(true)} />
            <span>
              <b>{t("drive_live")}</b> — {t("drive_live_hint")}
            </span>
          </label>
        </div>
        <Field label="Note (optional)">
          <Input value={note()} onInput={(e) => setNote(e.currentTarget.value)} maxLength={200} />
        </Field>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <div class="flex justify-end">
          <Button onClick={go} disabled={!to().trim()} loading={busy()}>
            <Share2 size={12} /> Share
          </Button>
        </div>
        <Show when={(shares() ?? []).length}>
          <div>
            <p class="label">Existing shares</p>
            <ul class="card divide-y divide-border">
              <For each={shares() ?? []}>
                {(s: ShareRecordView) => (
                  <li class="flex items-center gap-2 px-3 py-1.5 text-xs">
                    <span class="min-w-0 flex-1 truncate">
                      <Show when={s.grantee.startsWith("space:")} fallback={<Who address={s.grantee} size="sm" />}>
                        <span class="mono">Space {s.grantee.slice(6, 14)}…</span>
                      </Show>
                    </span>
                    <Badge brand={s.mode === "live"}>{s.mode}</Badge>
                    <span class="tnum text-muted">{shortWhen(s.granted_at_ms)}</span>
                    <Show when={!s.revoked} fallback={<Badge>{t("drive_revoked")}</Badge>}>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={async () => {
                          try {
                            await ipc.driveRevoke(s.share_id);
                            setTick((n) => n + 1);
                            store.toast("Revoked. The next version is sealed under a fresh key.");
                          } catch (e) {
                            store.toast(errText(e), "error");
                          }
                        }}
                      >
                        {t("drive_revoke")}
                      </Button>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>
      </div>
    </Dialog>
  );
}

function VersionsDialog(props: { entry: EntryView | null; onClose: () => void }) {
  const [tick, setTick] = createSignal(0);
  const [versions] = createResource(
    () => (props.entry ? { id: props.entry.id, tick: tick() } : null),
    (k) => ipc.driveVersions(k.id),
  );
  const current = () => versions()?.[versions()!.length - 1]?.version_no;
  return (
    <Dialog open={!!props.entry} onClose={props.onClose} title={`${t("drive_versions")} — ${props.entry?.name ?? ""}`} width="max-w-md">
      <ul class="card divide-y divide-border" data-testid="versions">
        <For each={[...(versions() ?? [])].reverse()} fallback={<li class="p-3 text-center text-xs text-muted">{versions.loading ? t("loading") : "no versions"}</li>}>
          {(v: VersionView) => (
            <li class="flex items-center gap-2 px-3 py-1.5 text-xs">
              <span class="tnum w-8">v{v.version_no}</span>
              <span class="tnum w-16 text-muted">{formatBytes(v.size)}</span>
              <span class="tnum flex-1 text-muted">{formatMs(v.created_at_ms)}</span>
              <span class="truncate text-muted">{v.note}</span>
              <Show when={v.version_no === current()}>
                <Badge brand>current</Badge>
              </Show>
              <Button
                variant="ghost"
                size="sm"
                title="Download this version"
                onClick={async () => {
                  const p = await pickSavePath(`${props.entry!.name}.v${v.version_no}`);
                  if (!p) return;
                  try {
                    await ipc.driveDownloadVersion(props.entry!.id, v.version_no, p);
                    store.toast("Saved");
                  } catch (e) {
                    store.toast(errText(e), "error");
                  }
                }}
              >
                <Download size={12} />
              </Button>
              <Show when={v.version_no !== current()}>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={async () => {
                    try {
                      await ipc.driveRestoreVersion(props.entry!.id, v.version_no);
                      setTick((n) => n + 1);
                      store.bump("drive");
                      store.toast(`Restored v${v.version_no} as current`);
                    } catch (e) {
                      store.toast(errText(e), "error");
                    }
                  }}
                >
                  {t("drive_restore")}
                </Button>
              </Show>
            </li>
          )}
        </For>
      </ul>
    </Dialog>
  );
}

function SharedWithMe(props: { items: SharedWithMeView[]; loading: boolean; onOpenFolder: (s: SharedWithMeView) => void; onChanged: () => void }) {
  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      store.bump("drive");
      props.onChanged();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  return (
    <Show when={props.items.length} fallback={<Empty title="Nothing shared with you yet" icon={<Users size={20} />}>{props.loading ? t("loading") : "Files and folders others share with you appear here, with their version and whether they were updated or revoked."}</Empty>}>
      <div class="flex h-full flex-col">
        <div class="grid grid-cols-[1fr_160px_80px_90px_auto] gap-2 border-b border-border px-3 py-1 text-[11px] text-muted">
          <span>Name</span>
          <span>Owner</span>
          <span>Version</span>
          <span>Received</span>
          <span />
        </div>
        <div class="min-h-0 flex-1 overflow-auto">
          <For each={props.items}>
            {(s) => (
              <div class="row grid grid-cols-[1fr_160px_80px_90px_auto] items-center gap-2 px-3 py-1.5 text-[13px]" data-share={s.capability.share_id}>
                <span class="flex min-w-0 items-center gap-2">
                  <Show when={s.capability.folder} fallback={<File size={14} class="text-muted" />}>
                    <Folder size={14} class="text-brand" />
                  </Show>
                  <span class="truncate">{s.capability.name}</span>
                  <Badge brand={s.capability.mode === "live"}>{s.capability.mode}</Badge>
                  <Show when={s.updates > 0}>
                    <Badge strong title={`${s.updates} update(s) received`}>
                      {t("mail_updated")}
                    </Badge>
                  </Show>
                  <Show when={s.revoked}>
                    <Badge title="The owner revoked this share; what you already downloaded stays with you.">{t("drive_revoked")}</Badge>
                  </Show>
                  <Show when={s.note}>
                    <span class="truncate text-xs text-muted">“{s.note}”</span>
                  </Show>
                </span>
                <span class="truncate text-xs">
                  <Who address={s.from} size="sm" />
                </span>
                <span class="tnum text-xs text-muted">v{s.capability.version_no}</span>
                <span class="tnum text-xs text-muted">{shortWhen(s.received_at_ms)}</span>
                <span class="flex items-center gap-1">
                  <Show when={s.capability.folder} fallback={
                    <>
                      <Button variant="ghost" size="sm" title={t("drive_open")} onClick={() => void act(() => ipc.driveSharedOpen(s.capability.share_id))}>
                        <ExternalLink size={12} />
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        title={t("drive_download")}
                        onClick={async () => {
                          const p = await pickSavePath(s.capability.name);
                          if (p) await act(() => ipc.driveSharedDownload(s.capability.share_id, p), "Saved");
                        }}
                      >
                        <Download size={12} />
                      </Button>
                      <Button variant="ghost" size="sm" title={t("drive_save_to_mine")} onClick={() => void act(() => ipc.driveSharedSave(s.capability.share_id, ""), "Saved to My Drive (no re-upload)")}>
                        <CopyIcon size={12} />
                      </Button>
                    </>
                  }>
                    <Button variant="ghost" size="sm" onClick={() => props.onOpenFolder(s)}>
                      Open folder
                    </Button>
                  </Show>
                </span>
              </div>
            )}
          </For>
        </div>
      </div>
    </Show>
  );
}

function SharedFolderDialog(props: { share: SharedWithMeView | null; onClose: () => void }) {
  const [entries] = createResource(
    () => props.share?.capability.share_id ?? null,
    (id) => ipc.driveSharedFolderList(id),
  );
  const files = createMemo(() => [...(entries() ?? [])].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "folder" ? -1 : 1)));
  return (
    <Dialog open={!!props.share} onClose={props.onClose} title={`Shared folder — ${props.share?.capability.name ?? ""}`} width="max-w-lg">
      <Show when={!entries.error} fallback={<ErrorState error={entries.error} compact />}>
        <ul class="card max-h-80 divide-y divide-border overflow-auto">
          <For each={files()} fallback={<li class="p-3 text-center text-xs text-muted">{entries.loading ? t("loading") : "empty folder"}</li>}>
            {(e: FolderEntryView) => (
              <li class="flex items-center gap-2 px-3 py-1.5 text-[13px]">
                <Show when={e.kind === "folder"} fallback={<File size={13} class="text-muted" />}>
                  <Folder size={13} class="text-brand" />
                </Show>
                <span class="min-w-0 flex-1 truncate">{e.name}</span>
                <span class="tnum text-xs text-muted">{e.kind === "file" ? formatBytes(e.size) : ""}</span>
                <Show when={e.kind === "file"}>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={async () => {
                      const p = await pickSavePath(e.name);
                      if (!p) return;
                      try {
                        await ipc.driveSharedFolderDownload(props.share!.capability.share_id, e.id, p);
                        store.toast("Saved");
                      } catch (err) {
                        store.toast(errText(err), "error");
                      }
                    }}
                  >
                    <Download size={12} />
                  </Button>
                </Show>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <p class="mt-2 text-[11px] text-muted">A folder share is a snapshot of the folder's manifest; files inside are downloaded one at a time.</p>
    </Dialog>
  );
}