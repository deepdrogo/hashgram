// Spaces: list with role badge; space view with Overview (announcements),
// Posts, Drive (path tree), Members (role management gated by my_role),
// Mail (to all members). Rule gating mirrors hashgram_app::space rules
// so controls are disabled before the SDK would refuse them.
import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { LayoutGrid, Plus, Megaphone, MessageSquare, Folder, File, Users, Mail, UserPlus, Send, Download, ExternalLink, Copy as CopyIcon, Crown, X, Share2 } from "lucide-solid";
import { Button, Dialog, Field, Input, Notice, Tabs, Textarea, Badge, Empty, Select, Checkbox } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Who, Avatar } from "~/components/identity";
import { ipc, errText, ROLE, type SpaceSummary, type SpaceStateView, type SpaceContentView, type SpaceMember, type SpaceSharedEntryView, type EntryView } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { roleName, shortWhen, formatBytes, formatMs } from "~/lib/format";
import { pickSavePath, confirm } from "~/lib/dialogs";

type Tab = "overview" | "posts" | "drive" | "members" | "mail";

/** Rule table from docs/SPACES.md, as predicates on my role. */
export const can = {
  post: (r: number) => r >= ROLE.member,
  comment: (r: number) => r >= ROLE.member,
  shareDrive: (r: number) => r >= ROLE.member,
  announce: (r: number) => r >= ROLE.admin,
  editInfo: (r: number) => r >= ROLE.admin,
  invite: (r: number) => r >= ROLE.admin,
  /** Which roles I may grant on invite. */
  inviteRoles: (r: number): number[] => (r === ROLE.owner ? [ROLE.guest, ROLE.member, ROLE.admin] : r === ROLE.admin ? [ROLE.guest, ROLE.member] : []),
  remove: (r: number, target: number) => r >= ROLE.admin && target < r,
  /** Roles I may set on a target (owner: any incl. transfer; admin: ≤ member on targets < admin). */
  setRoles: (r: number, target: number, isSelf: boolean): number[] => {
    if (r === ROLE.owner) return isSelf ? [] : [ROLE.guest, ROLE.member, ROLE.admin, ROLE.owner];
    if (r === ROLE.admin && target < ROLE.admin && !isSelf) return [ROLE.guest, ROLE.member];
    return [];
  },
  leave: (r: number) => r !== ROLE.owner && r !== ROLE.none,
  unshare: (r: number, sharedByMe: boolean) => sharedByMe || r >= ROLE.admin,
};

export function SpacesRoute() {
  const params = useParams<{ id?: string; tab?: string }>();
  const navigate = useNavigate();
  const [create, setCreate] = createSignal(false);
  const [list, { refetch }] = createResource(
    () => store.ticks().spaces,
    () => ipc.spacesList(),
  );
  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="flex min-h-0 flex-1">
        <aside class="pane w-[240px] shrink-0">
          <div class="p-2">
            <Button variant="brand" class="w-full" onClick={() => setCreate(true)}>
              <Plus size={14} /> {t("spaces_create")}
            </Button>
          </div>
          <div class="min-h-0 flex-1 overflow-auto">
            <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={() => void refetch()} compact />}>
              <For each={list() ?? []} fallback={<Empty title="No Spaces yet" icon={<LayoutGrid size={18} />}>A Space is a shared environment with roles: family, company, project.</Empty>}>
                {(s) => (
                  <button
                    type="button"
                    class={`row flex w-full items-center gap-2 border-b border-border px-3 py-2 text-left ${params.id === s.id ? "bg-surface-2" : ""}`}
                    onClick={() => navigate(`/spaces/${s.id}`)}
                    data-space={s.id}
                  >
                    <span class="min-w-0 flex-1">
                      <span class="block truncate text-[13px] font-medium">{s.name}</span>
                      <span class="block truncate text-xs text-muted">{s.members} member{s.members === 1 ? "" : "s"}</span>
                    </span>
                    <Badge brand={s.my_role === ROLE.owner} strong={s.my_role === ROLE.admin}>
                      {roleName(s.my_role)}
                    </Badge>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </aside>
        <section class="min-w-0 flex-1 overflow-hidden">
          <Show when={params.id} fallback={<div class="flex h-full items-center justify-center text-sm text-muted">Pick a Space</div>}>
            {(id) => <SpaceView id={id()} tab={(params.tab as Tab) || "overview"} onTab={(tb) => navigate(`/spaces/${id()}/${tb}`)} />}
          </Show>
        </section>
      </div>
      <CreateSpaceDialog open={create()} onClose={() => setCreate(false)} onCreated={(id) => navigate(`/spaces/${id}`)} />
    </div>
  );
}

function CreateSpaceDialog(props: { open: boolean; onClose: () => void; onCreated: (id: string) => void }) {
  const [name, setName] = createSignal("");
  const [desc, setDesc] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("spaces_create")} description="You become the Owner. Invite people with a role from Members." width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="Name">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={128} />
        </Field>
        <Field label="Description">
          <Textarea value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} rows={2} />
        </Field>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <div class="flex justify-end">
          <Button
            loading={busy()}
            disabled={!name().trim()}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const id = await ipc.spacesCreate(name(), desc());
                store.bump("spaces");
                setName(""); setDesc("");
                props.onClose();
                props.onCreated(id);
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

function SpaceView(props: { id: string; tab: Tab; onTab: (t: Tab) => void }) {
  const [state, { refetch }] = createResource(
    () => ({ id: props.id, tick: store.ticks().spaces }),
    (k) => ipc.spacesState(k.id),
  );
  const [content, { refetch: refetchContent }] = createResource(
    () => ({ id: props.id, tick: store.ticks().spaces }),
    (k) => ipc.spacesContent(k.id, 0, 200),
  );
  const role = () => state()?.my_role ?? ROLE.none;
  const me = () => store.status()?.address ?? "";
  const [invite, setInvite] = createSignal(false);
  const [editInfo, setEditInfo] = createSignal(false);
  const [shareDrive, setShareDrive] = createSignal(false);
  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      store.bump("spaces");
      void refetch();
      void refetchContent();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  const announcements = createMemo(() => (content() ?? []).filter((c) => c.kind === "announcement"));
  const posts = createMemo(() => (content() ?? []).filter((c) => c.kind === "post"));
  const commentsOf = (id: string) => (content() ?? []).filter((c) => c.kind === "comment" && c.post_id === id).reverse();

  return (
    <Show when={!state.error} fallback={<ErrorState error={state.error} onRetry={() => void refetch()} />}>
      <Show when={state()} fallback={<div class="p-6 text-xs text-muted">{t("loading")}</div>}>
        {(s) => (
          <div class="flex h-full flex-col">
            <header class="flex items-start gap-3 border-b border-border px-4 pt-3 pb-0">
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <h1 class="truncate text-base font-semibold">{s().name}</h1>
                  <Badge brand={role() === ROLE.owner} strong={role() === ROLE.admin}>
                    {roleName(role())}
                  </Badge>
                  <Show when={can.editInfo(role())}>
                    <button type="button" class="text-xs text-muted hover:text-fg" onClick={() => setEditInfo(true)}>
                      edit
                    </button>
                  </Show>
                </div>
                <p class="truncate text-xs text-muted">{s().description || `${s().members.length} members · created ${formatMs(s().created_at_ms)}`}</p>
                <Tabs
                  class="mt-2 border-b-0"
                  value={props.tab}
                  onChange={(v) => props.onTab(v as Tab)}
                  tabs={[
                    { id: "overview", label: t("spaces_overview"), badge: announcements().length },
                    { id: "posts", label: t("spaces_posts"), badge: posts().length },
                    { id: "drive", label: t("spaces_drive"), badge: s().drive.length },
                    { id: "members", label: t("spaces_members"), badge: s().members.length },
                    { id: "mail", label: t("spaces_mail") },
                  ]}
                />
              </div>
              <div class="flex shrink-0 items-center gap-1.5 pb-2">
                <Button variant="secondary" size="sm" onClick={() => setInvite(true)} disabled={!can.invite(role())} title={can.invite(role()) ? "" : "Admins and the Owner invite"}>
                  <UserPlus size={12} /> {t("spaces_invite")}
                </Button>
                <Show when={can.leave(role())}>
                  <Button variant="ghost" size="sm" onClick={async () => { if (await confirm(`Leave "${s().name}"?`)) await act(() => ipc.spacesRemove(props.id, me(), "left"), "You left the Space"); }}>
                    Leave
                  </Button>
                </Show>
              </div>
            </header>
            <div class="min-h-0 flex-1 overflow-auto">
              <Show when={props.tab === "overview"}>
                <div class="mx-auto max-w-2xl px-4 py-3">
                  <Show when={can.announce(role())} fallback={<p class="mb-3 text-[11px] text-muted">Announcements are written by Admins and the Owner.</p>}>
                    <AnnounceForm onSubmit={(title, text) => act(() => ipc.spacesAnnounce(props.id, title, text), "Announced")} />
                  </Show>
                  <For each={announcements()} fallback={<Empty title="No announcements yet" icon={<Megaphone size={18} />} />}>
                    {(a) => (
                      <article class="card mb-2 p-3">
                        <div class="flex items-center gap-2 text-xs text-muted">
                          <Megaphone size={11} class="text-brand" />
                          <Who address={a.actor} size="sm" />
                          <span>{shortWhen(a.at_ms)}</span>
                        </div>
                        <h3 class="mt-1 text-[13px] font-semibold">{a.title}</h3>
                        <p class="mt-1 whitespace-pre-wrap text-[13px] selectable">{a.text}</p>
                      </article>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={props.tab === "posts"}>
                <div class="mx-auto max-w-2xl px-4 py-3">
                  <Show when={can.post(role())} fallback={<Notice class="mb-3">Guests read only. Ask an Admin for the Member role to post.</Notice>}>
                    <PostForm placeholder="Post to the Space…" onSubmit={(text) => act(() => ipc.spacesPost(props.id, text))} />
                  </Show>
                  <For each={posts()} fallback={<Empty title="No posts yet" icon={<MessageSquare size={18} />} />}>
                    {(p) => (
                      <article class="card mb-2 p-3 text-[13px]" data-space-post={p.id}>
                        <div class="flex items-center gap-2">
                          <Avatar address={p.actor} size={22} />
                          <span class="font-medium"><Who address={p.actor} /></span>
                          <span class="flex-1" />
                          <span class="tnum text-[11px] text-muted">{shortWhen(p.at_ms)}</span>
                        </div>
                        <p class="mt-2 whitespace-pre-wrap selectable">{p.text}</p>
                        <Show when={p.drive_refs.length}>
                          <div class="mt-2 flex flex-wrap gap-1.5">
                            <For each={p.drive_refs}>{(c) => <Badge><File size={10} class="mr-1" />{c.name}</Badge>}</For>
                          </div>
                        </Show>
                        <div class="mt-2 border-t border-border pt-2">
                          <For each={commentsOf(p.id)}>
                            {(c) => (
                              <div class="mb-1 text-xs">
                                <span class="mr-1 font-medium"><Who address={c.actor} size="sm" /></span>
                                <span class="selectable">{c.text}</span>
                              </div>
                            )}
                          </For>
                          <Show when={can.comment(role())}>
                            <PostForm compact placeholder="Comment…" onSubmit={(text) => act(() => ipc.spacesComment(props.id, p.id, text))} />
                          </Show>
                        </div>
                      </article>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={props.tab === "drive"}>
                <SpaceDrive space={props.id} entries={s().drive} role={role()} me={me()} onShare={() => setShareDrive(true)} onChanged={() => { store.bump("spaces"); void refetch(); }} />
              </Show>
              <Show when={props.tab === "members"}>
                <Members space={props.id} state={s()} me={me()} onChanged={() => { store.bump("spaces"); void refetch(); }} />
              </Show>
              <Show when={props.tab === "mail"}>
                <SpaceMail space={props.id} members={s().members.length} />
              </Show>
            </div>
            <InviteDialog open={invite()} onClose={() => setInvite(false)} space={props.id} roles={can.inviteRoles(role())} onDone={() => { store.bump("spaces"); void refetch(); }} />
            <EditInfoDialog open={editInfo()} onClose={() => setEditInfo(false)} space={props.id} name={s().name} description={s().description} onDone={() => void refetch()} />
            <ShareToSpaceDialog open={shareDrive()} onClose={() => setShareDrive(false)} space={props.id} onDone={() => { store.bump("spaces"); void refetch(); }} />
          </div>
        )}
      </Show>
    </Show>
  );
}

function AnnounceForm(props: { onSubmit: (title: string, text: string) => Promise<void> }) {
  const [title, setTitle] = createSignal("");
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  return (
    <div class="card mb-3 flex flex-col gap-2 p-3">
      <Input value={title()} onInput={(e) => setTitle(e.currentTarget.value)} placeholder="Announcement title" maxLength={128} />
      <Textarea value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder="Text" rows={2} />
      <div class="flex justify-end">
        <Button size="sm" loading={busy()} disabled={!title().trim() && !text().trim()} onClick={async () => { setBusy(true); try { await props.onSubmit(title(), text()); setTitle(""); setText(""); } finally { setBusy(false); } }}>
          <Megaphone size={12} /> Announce
        </Button>
      </div>
    </div>
  );
}

function PostForm(props: { placeholder: string; compact?: boolean; onSubmit: (text: string) => Promise<void> }) {
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const go = async () => {
    if (!text().trim()) return;
    setBusy(true);
    try {
      await props.onSubmit(text());
      setText("");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class={`flex items-start gap-2 ${props.compact ? "mt-1" : "card mb-3 p-3"}`}>
      <Show when={props.compact} fallback={<Textarea value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder={props.placeholder} rows={2} />}>
        <Input class="h-7" value={text()} onInput={(e) => setText(e.currentTarget.value)} placeholder={props.placeholder} onKeyDown={(e) => e.key === "Enter" && void go()} />
      </Show>
      <Button size="sm" loading={busy()} disabled={!text().trim()} onClick={go}>
        <Send size={12} />
      </Button>
    </div>
  );
}

function SpaceDrive(props: { space: string; entries: SpaceSharedEntryView[]; role: number; me: string; onShare: () => void; onChanged: () => void }) {
  const tree = createMemo(() => {
    const groups = new Map<string, SpaceSharedEntryView[]>();
    for (const e of props.entries) {
      const k = e.path || "/";
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k)!.push(e);
    }
    return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
  });
  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      props.onChanged();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  return (
    <div class="mx-auto max-w-3xl px-4 py-3">
      <div class="mb-3 flex items-center gap-2">
        <Button variant="secondary" size="sm" onClick={props.onShare} disabled={!can.shareDrive(props.role)} title={can.shareDrive(props.role) ? "" : "Members and above share files"}>
          <Share2 size={12} /> Share a file from my Drive
        </Button>
        <span class="text-[11px] text-muted">Live shares follow the owner's edits.</span>
      </div>
      <For each={tree()} fallback={<Empty title="The Space drive is empty" icon={<Folder size={18} />}>Members share files from their own Drive into a path here.</Empty>}>
        {([path, items]) => (
          <div class="mb-3">
            <p class="mb-1 flex items-center gap-1 text-xs font-medium text-muted">
              <Folder size={12} class="text-brand" /> {path}
            </p>
            <ul class="card divide-y divide-border">
              <For each={items}>
                {(e) => (
                  <li class="flex items-center gap-2 px-3 py-1.5 text-[13px]" data-space-share={e.capability.share_id}>
                    <Show when={e.capability.folder} fallback={<File size={13} class="text-muted" />}>
                      <Folder size={13} class="text-brand" />
                    </Show>
                    <span class="min-w-0 flex-1 truncate">{e.capability.name}</span>
                    <Badge brand={e.capability.mode === "live"}>{e.capability.mode} v{e.capability.version_no}</Badge>
                    <span class="tnum text-xs text-muted">{formatBytes(e.capability.size)}</span>
                    <span class="text-xs text-muted"><Who address={e.by} size="sm" /></span>
                    <Show when={!e.capability.folder}>
                      <Button variant="ghost" size="sm" title={t("drive_open")} onClick={() => void act(() => ipc.spacesDriveOpen(props.space, e.capability.share_id))}>
                        <ExternalLink size={12} />
                      </Button>
                      <Button variant="ghost" size="sm" title={t("drive_download")} onClick={async () => { const p = await pickSavePath(e.capability.name); if (p) await act(() => ipc.spacesDriveDownload(props.space, e.capability.share_id, p), "Saved"); }}>
                        <Download size={12} />
                      </Button>
                      <Button variant="ghost" size="sm" title={t("drive_save_to_mine")} onClick={() => void act(() => ipc.spacesDriveSave(props.space, e.capability.share_id, ""), "Saved to My Drive")}>
                        <CopyIcon size={12} />
                      </Button>
                    </Show>
                    <Show when={can.unshare(props.role, e.by === props.me)}>
                      <Button variant="ghost" size="sm" title="Unshare" onClick={() => void act(() => ipc.spacesUnshareDrive(props.space, e.capability.share_id), "Unshared")}>
                        <X size={12} />
                      </Button>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        )}
      </For>
    </div>
  );
}

function Members(props: { space: string; state: SpaceStateView; me: string; onChanged: () => void }) {
  const my = () => props.state.my_role;
  const act = async (f: () => Promise<unknown>, done?: string) => {
    try {
      await f();
      if (done) store.toast(done);
      props.onChanged();
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  const roleKey = (r: number) => ["", "guest", "member", "admin", "owner"][r] ?? "member";
  return (
    <div class="mx-auto max-w-3xl px-4 py-3">
      <ul class="card divide-y divide-border" data-testid="members">
        <For each={props.state.members}>
          {(m: SpaceMember) => {
            const isSelf = () => m.address === props.me;
            const options = () => can.setRoles(my(), m.role, isSelf());
            return (
              <li class="flex items-center gap-3 px-3 py-2 text-[13px]" data-member={m.address} data-role={m.role}>
                <Avatar address={m.address} size={24} />
                <span class="min-w-0 flex-1 truncate"><Who address={m.address} me={isSelf()} /></span>
                <span class="tnum text-xs text-muted">since {shortWhen(m.since_ms)}</span>
                <Show when={m.role === ROLE.owner}>
                  <Crown size={12} class="text-brand" />
                </Show>
                <Show when={options().length} fallback={<Badge brand={m.role === ROLE.owner} strong={m.role === ROLE.admin}>{roleName(m.role)}</Badge>}>
                  <Select
                    class="w-28"
                    value={roleKey(m.role)}
                    aria-label="role"
                    options={[...new Set([m.role, ...options()])].sort().map((r) => ({ value: roleKey(r), label: roleName(r) }))}
                    onChange={async (v) => {
                      if (v === roleKey(m.role)) return;
                      if (v === "owner" && !(await confirm(`Transfer ownership to this member? You become an Admin and can then leave.`))) return;
                      await act(() => ipc.spacesSetRole(props.space, m.address, v), "Role changed");
                    }}
                  />
                </Show>
                <Show when={!isSelf() && can.remove(my(), m.role)}>
                  <Button variant="ghost" size="sm" title="Remove from the Space" onClick={async () => { if (await confirm(`Remove this member? They stop receiving from the next message on.`)) await act(() => ipc.spacesRemove(props.space, m.address, "removed by admin"), "Removed"); }}>
                    <X size={12} />
                  </Button>
                </Show>
              </li>
            );
          }}
        </For>
      </ul>
      <p class="mt-2 text-[11px] text-muted">
        Guests read. Members post, comment and share files. Admins invite (up to Member; Admins only by the Owner), announce, edit info and remove lower roles. The Owner transfers ownership from here and cannot be removed.
      </p>
    </div>
  );
}

function InviteDialog(props: { open: boolean; onClose: () => void; space: string; roles: number[]; onDone: () => void }) {
  const [who, setWho] = createSignal("");
  const [role, setRole] = createSignal("member");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [preview] = createResource(
    () => who().trim(),
    (w) => (w.length >= 2 ? ipc.peopleResolve(w).catch(() => null) : Promise.resolve(null)),
  );
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("spaces_invite")} width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="Who" hint={preview() ? `${preview()!.address} · ${preview()!.devices} device(s)` : "@name, name@hashgram.io or address"}>
          <Input value={who()} onInput={(e) => setWho(e.currentTarget.value)} />
        </Field>
        <Field label="Role">
          <Select value={role()} onChange={setRole} options={props.roles.map((r) => ({ value: ["", "guest", "member", "admin", "owner"][r]!, label: roleName(r) }))} />
        </Field>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <div class="flex justify-end">
          <Button
            loading={busy()}
            disabled={!who().trim() || !props.roles.length}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await ipc.spacesInvite(props.space, who(), role());
                store.toast("Invited");
                setWho("");
                props.onDone();
                props.onClose();
              } catch (e) {
                setError(errText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            <UserPlus size={12} /> Invite
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function EditInfoDialog(props: { open: boolean; onClose: () => void; space: string; name: string; description: string; onDone: () => void }) {
  const [name, setName] = createSignal(props.name);
  const [desc, setDesc] = createSignal(props.description);
  const [busy, setBusy] = createSignal(false);
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Edit Space" width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="Name">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={128} />
        </Field>
        <Field label="Description">
          <Textarea value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} rows={2} />
        </Field>
        <div class="flex justify-end">
          <Button loading={busy()} onClick={async () => { setBusy(true); try { await ipc.spacesSetInfo(props.space, name(), desc()); props.onDone(); props.onClose(); } catch (e) { store.toast(errText(e), "error"); } finally { setBusy(false); } }}>
            {t("save")}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function ShareToSpaceDialog(props: { open: boolean; onClose: () => void; space: string; onDone: () => void }) {
  const [parent, setParent] = createSignal("");
  const [picked, setPicked] = createSignal<EntryView | null>(null);
  const [path, setPath] = createSignal("");
  const [live, setLive] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [entries] = createResource(
    () => (props.open ? parent() : null),
    (p) => ipc.driveList(p),
  );
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Share to the Space drive" width="max-w-md">
      <div class="flex flex-col gap-3">
        <div class="card max-h-56 overflow-auto">
          <Show when={parent()}>
            <button type="button" class="row w-full px-3 py-1.5 text-left text-xs text-muted" onClick={() => setParent("")}>
              ← My Drive
            </button>
          </Show>
          <For each={entries() ?? []} fallback={<div class="p-3 text-center text-xs text-muted">{entries.loading ? t("loading") : "empty"}</div>}>
            {(e) => (
              <button type="button" class={`row flex w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] ${picked()?.id === e.id ? "bg-surface-2" : ""}`} onClick={() => (e.kind === "folder" ? setParent(e.id) : setPicked(e))} onDblClick={() => e.kind === "folder" && setParent(e.id)}>
                <Show when={e.kind === "folder"} fallback={<File size={13} class="text-muted" />}>
                  <Folder size={13} class="text-brand" />
                </Show>
                <span class="flex-1 truncate">{e.name}</span>
                <span class="tnum text-xs text-muted">{e.kind === "file" ? formatBytes(e.size) : ""}</span>
              </button>
            )}
          </For>
        </div>
        <Field label="Path inside the Space drive" hint='e.g. "Contracts/2026"'>
          <Input value={path()} onInput={(e) => setPath(e.currentTarget.value)} />
        </Field>
        <Checkbox checked={live()} onChange={setLive} label={`${t("drive_live")} — ${t("drive_live_hint")}`} />
        <div class="flex justify-end">
          <Button loading={busy()} disabled={!picked()} onClick={async () => { setBusy(true); try { await ipc.spacesShareDrive(props.space, picked()!.id, path(), live()); store.toast("Shared to the Space"); props.onDone(); props.onClose(); } catch (e) { store.toast(errText(e), "error"); } finally { setBusy(false); } }}>
            <Share2 size={12} /> Share {picked()?.name ?? ""}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function SpaceMail(props: { space: string; members: number }) {
  const [subject, setSubject] = createSignal("");
  const [body, setBody] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  return (
    <div class="mx-auto max-w-2xl px-4 py-3">
      <p class="mb-2 text-xs text-muted">
        <Mail size={11} class="mr-1 inline" /> One mail to all {props.members - 1} other member{props.members - 1 === 1 ? "" : "s"}, labelled with this Space. Replies land in your Inbox.
      </p>
      <div class="card flex flex-col gap-2 p-3">
        <Input value={subject()} onInput={(e) => setSubject(e.currentTarget.value)} placeholder={t("mail_subject")} />
        <Textarea value={body()} onInput={(e) => setBody(e.currentTarget.value)} rows={6} placeholder="Message" />
        <div class="flex justify-end">
          <Button loading={busy()} disabled={!subject().trim() && !body().trim()} onClick={async () => { setBusy(true); try { await ipc.spacesMail(props.space, subject(), body()); store.toast("Sent to the Space"); setSubject(""); setBody(""); store.bump("mail"); } catch (e) { store.toast(errText(e), "error"); } finally { setBusy(false); } }}>
            <Send size={12} /> {t("send")}
          </Button>
        </div>
      </div>
    </div>
  );
}

export type { SpaceSummary, SpaceContentView };
