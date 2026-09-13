// People: search (username / mail address / hash1… / local contacts),
// lists (Friends, Requests, Following, Blocked), the contact card and the
// user's own profile editors (public via Feed, private display name).
import { For, Show, createEffect, createResource, createSignal } from "solid-js";
import { useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { Search, UserPlus, Mail, Check, X, Ban, ShieldCheck, VolumeX, Heart, IdCard, Users, Copy as CopyIcon } from "lucide-solid";
import { Button, Checkbox, Dialog, Field, Input, Notice, Tabs, Textarea, Badge, Empty } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Avatar, Mono, forgetAvatar } from "~/components/identity";
import { ipc, errText, type ContactRecord, type Resolved } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { handle, formatMs } from "~/lib/format";
import { copyText } from "~/lib/clipboard";
import { pickFile, confirm } from "~/lib/dialogs";

type ListKind = "friends" | "incoming" | "following" | "blocked";

export function PeopleRoute() {
  const params = useParams<{ address?: string }>();
  const [search, setSearch] = useSearchParams<{ q?: string }>();
  const navigate = useNavigate();
  const [tab, setTab] = createSignal<ListKind>("friends");
  const [q, setQ] = createSignal(search.q ?? "");
  const [resolved, setResolved] = createSignal<Resolved | null>(null);
  const [resolveError, setResolveError] = createSignal<string | null>(null);
  const [resolving, setResolving] = createSignal(false);
  const [me, setMe] = createSignal(false);

  const [list, { refetch }] = createResource(
    () => ({ tab: tab(), tick: store.ticks().people }),
    (k) => ipc.peopleList(k.tab),
  );
  const [outgoing] = createResource(
    () => ({ tab: tab(), tick: store.ticks().people }),
    (k) => (k.tab === "incoming" ? ipc.peopleList("outgoing") : Promise.resolve([] as ContactRecord[])),
  );
  const [local] = createResource(
    () => q().trim(),
    (query) => (query.length >= 2 ? ipc.peopleSearchLocal(query) : Promise.resolve([] as ContactRecord[])),
  );

  const lookup = async () => {
    const query = q().trim();
    if (!query) return;
    setResolving(true);
    setResolveError(null);
    setResolved(null);
    try {
      const r = await ipc.peopleResolve(query);
      setResolved(r);
      void ipc.searchNote(query).catch(() => undefined);
    } catch (e) {
      setResolveError(errText(e));
    } finally {
      setResolving(false);
    }
  };
  createEffect(() => {
    if (search.q) {
      setQ(search.q);
      void lookup();
      setSearch({ q: undefined });
    }
  });

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="flex min-h-0 flex-1">
        <aside class="pane w-[320px] shrink-0">
          <div class="flex items-center gap-2 border-b border-border p-2">
            <Search size={13} class="text-muted" />
            <Input class="h-7 border-0 bg-transparent px-0" placeholder="@name, name@hashgram.io, hash1…" value={q()} onInput={(e) => setQ(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void lookup()} />
            <Button size="sm" variant="secondary" onClick={lookup} loading={resolving()} disabled={!q().trim()}>
              Look up
            </Button>
          </div>
          <Show when={resolveError()}>
            <Notice strong class="m-2">
              {resolveError()}
            </Notice>
          </Show>
          <Show when={resolved()}>
            {(r) => (
              <button type="button" class="row flex w-full items-center gap-3 border-b border-border px-3 py-2 text-left" onClick={() => navigate(`/people/${r().address}`)}>
                <Avatar address={r().address} />
                <span class="min-w-0 flex-1">
                  <span class="block truncate text-[13px] font-medium">{handle(r().address, r().username, r().display_name)}</span>
                  <span class="block truncate text-xs text-muted">
                    {r().has_identity ? `${r().devices} device${r().devices === 1 ? "" : "s"} on chain` : "not registered on chain yet"}
                    {r().mail_address ? ` · ${r().mail_address}` : ""}
                  </span>
                </span>
                <Badge brand>network</Badge>
              </button>
            )}
          </Show>
          <Show when={(local() ?? []).length}>
            <For each={local() ?? []}>{(c) => <ContactRow c={c} onOpen={() => navigate(`/people/${c.address}`)} />}</For>
          </Show>
          <Tabs
            class="px-2"
            value={tab()}
            onChange={(v) => setTab(v as ListKind)}
            tabs={[
              { id: "friends", label: t("people_friends") },
              { id: "incoming", label: t("people_requests"), badge: store.requestsIn() },
              { id: "following", label: t("people_following") },
              { id: "blocked", label: t("people_blocked") },
            ]}
          />
          <div class="min-h-0 flex-1 overflow-auto">
            <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={() => void refetch()} compact />}>
              <Show when={(list() ?? []).length || (outgoing() ?? []).length} fallback={<Empty title={t("nothing_here")} icon={<Users size={18} />}>{tab() === "friends" ? "Look someone up above and send a contact request." : ""}</Empty>}>
                <Show when={tab() === "incoming" && (list() ?? []).length}>
                  <p class="px-3 pt-2 text-[11px] uppercase tracking-wide text-muted">Incoming</p>
                </Show>
                <For each={list() ?? []}>
                  {(c) => (
                    <ContactRow
                      c={c}
                      onOpen={() => navigate(`/people/${c.address}`)}
                      actions={
                        tab() === "incoming" ? (
                          <>
                            <Button size="sm" variant="brand" onClick={(e) => { e.stopPropagation(); void ipc.peopleRespond(c.address, true).then(() => { store.bump("people"); store.refreshCounts(); store.toast("Contact added"); }).catch((err) => store.toast(errText(err), "error")); }}>
                              <Check size={12} /> {t("people_accept")}
                            </Button>
                            <Button size="sm" variant="ghost" onClick={(e) => { e.stopPropagation(); void ipc.peopleRespond(c.address, false).then(() => { store.bump("people"); store.refreshCounts(); }).catch((err) => store.toast(errText(err), "error")); }}>
                              <X size={12} />
                            </Button>
                          </>
                        ) : undefined
                      }
                    />
                  )}
                </For>
                <Show when={tab() === "incoming" && (outgoing() ?? []).length}>
                  <p class="px-3 pt-3 text-[11px] uppercase tracking-wide text-muted">Sent, awaiting answer</p>
                  <For each={outgoing() ?? []}>{(c) => <ContactRow c={c} onOpen={() => navigate(`/people/${c.address}`)} />}</For>
                </Show>
              </Show>
            </Show>
          </div>
          <div class="border-t border-border p-2">
            <Button variant="ghost" size="sm" class="w-full" onClick={() => setMe(true)}>
              <IdCard size={13} /> My profile
            </Button>
          </div>
        </aside>
        <section class="min-w-0 flex-1 overflow-auto">
          <Show when={params.address} fallback={<div class="flex h-full items-center justify-center text-sm text-muted">Pick a person or look one up</div>}>
            {(a) => <ContactCard address={a()} />}
          </Show>
        </section>
      </div>
      <MyProfileDialog open={me()} onClose={() => setMe(false)} />
    </div>
  );
}

function ContactRow(props: { c: ContactRecord; onOpen: () => void; actions?: any }) {
  return (
    <div class="row flex cursor-default items-center gap-3 border-b border-border px-3 py-2" onClick={props.onOpen}>
      <Avatar address={props.c.address} />
      <span class="min-w-0 flex-1">
        <span class="block truncate text-[13px] font-medium">{handle(props.c.address, props.c.username, props.c.display_name)}</span>
        <span class="mono block truncate text-xs text-muted">{props.c.username ? `@${props.c.username}` : props.c.address}</span>
      </span>
      <Show when={props.actions}>
        <span class="flex items-center gap-1">{props.actions}</span>
      </Show>
    </div>
  );
}

function ContactCard(props: { address: string }) {
  const navigate = useNavigate();
  const [tick, setTick] = createSignal(0);
  const [profile, { refetch }] = createResource(
    () => ({ a: props.address, tick: tick(), t2: store.ticks().people }),
    (k) => ipc.peopleProfile(k.a),
  );
  const [resolved] = createResource(
    () => props.address,
    (a) => ipc.peopleResolve(a).catch(() => null),
  );
  const [card] = createResource(
    () => ({ a: props.address, tick: tick() }),
    (k) => ipc.peopleCardOf(k.a).catch(() => null),
  );
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [sendCard, setSendCard] = createSignal(false);
  const has = (s: string) => profile()?.states.includes(s) ?? false;
  const isMe = () => store.status()?.address === props.address;
  const act = async (f: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await f();
      if (done) store.toast(done);
      setTick((n) => n + 1);
      store.bump("people");
      void store.refreshCounts();
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={!profile.error} fallback={<ErrorState error={profile.error} onRetry={() => void refetch()} />}>
      <div class="page max-w-3xl">
        <div class="flex items-start gap-4">
          <Avatar address={props.address} size={56} />
          <div class="min-w-0 flex-1">
            <h1 class="truncate text-lg font-semibold">{handle(props.address, profile()?.username, profile()?.display_name)}</h1>
            <div class="mt-0.5 flex flex-wrap items-center gap-2 text-xs text-muted">
              <Show when={profile()?.username}>
                <span class="mono">@{profile()?.username}</span>
                <span>·</span>
                <span class="mono">{profile()?.username}@hashgram.io</span>
                <span>·</span>
              </Show>
              <Mono text={props.address} head={12} tail={6} copy />
            </div>
            <div class="mt-1 flex flex-wrap gap-1.5">
              <Show when={has("friend")}>
                <Badge brand>contact</Badge>
              </Show>
              <Show when={has("pending_out")}>
                <Badge>request sent</Badge>
              </Show>
              <Show when={has("pending_in")}>
                <Badge strong>wants to connect</Badge>
              </Show>
              <Show when={has("following")}>
                <Badge>following</Badge>
              </Show>
              <Show when={has("trusted")}>
                <Badge brand>trusted</Badge>
              </Show>
              <Show when={has("muted")}>
                <Badge>muted</Badge>
              </Show>
              <Show when={has("blocked")}>
                <Badge strong>blocked</Badge>
              </Show>
              <Show when={resolved()}>
                {(r) => (
                  <Badge title="devices registered on chain">
                    {r().has_identity ? `${r().devices} device${r().devices === 1 ? "" : "s"}` : "not registered"}
                  </Badge>
                )}
              </Show>
            </div>
            <Show when={profile()?.bio}>
              <p class="mt-2 max-w-xl text-[13px] selectable">{profile()?.bio}</p>
            </Show>
          </div>
        </div>

        <Show when={!isMe()}>
          <div class="mt-4 flex flex-wrap items-center gap-2">
            <Button variant="brand" onClick={() => navigate(`/mail/inbox?compose=1&q=${encodeURIComponent(profile()?.username ? `@${profile()!.username}` : props.address)}`)} disabled={has("blocked")}>
              <Mail size={14} /> {t("people_mail")}
            </Button>
            <Show when={has("pending_in")}>
              <Button onClick={() => act(() => ipc.peopleRespond(props.address, true), "Contact added")} loading={busy()}>
                <Check size={14} /> {t("people_accept")}
              </Button>
              <Button variant="secondary" onClick={() => act(() => ipc.peopleRespond(props.address, false))} loading={busy()}>
                <X size={14} /> {t("people_reject")}
              </Button>
            </Show>
            <Show when={!has("friend") && !has("pending_out") && !has("pending_in") && !has("blocked")}>
              <div class="flex items-center gap-1.5">
                <Input class="w-56" placeholder="say who you are (optional)" value={message()} onInput={(e) => setMessage(e.currentTarget.value)} maxLength={1024} />
                <Button variant="secondary" onClick={() => act(() => ipc.peopleRequest(props.address, message()), "Request sent")} loading={busy()} disabled={!resolved()?.has_identity} title={resolved() && !resolved()!.has_identity ? "They have no device on chain yet" : ""}>
                  <UserPlus size={14} /> {t("people_add")}
                </Button>
              </div>
            </Show>
            <Show when={has("friend")}>
              <Button variant="secondary" onClick={() => setSendCard(true)}>
                <IdCard size={14} /> {t("people_send_card")}
              </Button>
              <Button variant="ghost" onClick={async () => { if (await confirm("Remove this contact? Their mail goes back to Requests.")) await act(() => ipc.peopleRemove(props.address), "Removed"); }}>
                Remove contact
              </Button>
            </Show>
            <Button variant={has("following") ? "secondary" : "ghost"} onClick={() => act(() => ipc.peopleFollow(props.address, !has("following")))} loading={busy()} title="Following is public: anyone can see who follows whom.">
              <Heart size={14} fill={has("following") ? "currentColor" : "none"} /> {has("following") ? t("people_unfollow") : t("people_follow")}
            </Button>
          </div>
          <div class="mt-3 flex flex-wrap items-center gap-2 text-xs">
            <Checkbox checked={has("trusted")} onChange={(v) => void act(() => ipc.peopleTrust(props.address, v))} label={t("people_trust")} hint="Never filtered." />
            <Checkbox checked={has("muted")} onChange={(v) => void act(() => ipc.peopleMute(props.address, v))} label={t("people_mute")} hint="No notifications; mail still arrives." />
            <span class="flex-1" />
            <Show when={!has("blocked")} fallback={<Button variant="secondary" size="sm" onClick={() => act(() => ipc.peopleUnblock(props.address), "Unblocked")}>{t("people_unblock")}</Button>}>
              <Button variant="danger" size="sm" onClick={async () => { if (await confirm("Block this address? Everything from them is dropped.")) await act(() => ipc.peopleBlock(props.address), "Blocked"); }}>
                <Ban size={12} /> {t("people_block")}
              </Button>
            </Show>
          </div>
        </Show>

        <Show when={card()}>
          {(c) => (
            <div class="card mt-5 p-3 text-[13px]">
              <p class="mb-1 flex items-center gap-1 text-[11px] uppercase tracking-wide text-muted">
                <ShieldCheck size={11} /> Their card, sent privately to you · {formatMs(c().at_ms)}
              </p>
              <Show when={c().display_name}>
                <p class="font-medium">{c().display_name}</p>
              </Show>
              <Show when={c().bio}>
                <p class="selectable">{c().bio}</p>
              </Show>
              <Show when={c().wallet_address}>
                <p class="mt-1 flex items-center gap-1 text-xs text-muted">
                  Wallet they disclosed: <Mono text={c().wallet_address} copy />
                </p>
              </Show>
            </div>
          )}
        </Show>

        <div class="mt-6">
          <h2 class="mb-2 text-[13px] font-semibold">Public posts</h2>
          <AuthorFeed address={props.address} />
        </div>
      </div>
      <SendCardDialog open={sendCard()} onClose={() => setSendCard(false)} address={props.address} />
    </Show>
  );
}

function SendCardDialog(props: { open: boolean; onClose: () => void; address: string }) {
  const [bio, setBio] = createSignal("");
  const [wallet, setWallet] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  return (
    <Dialog open={props.open} onClose={props.onClose} title={t("people_send_card")} width="max-w-md">
      <div class="flex flex-col gap-3">
        <Field label="A line about you (private, only to this contact)">
          <Textarea value={bio()} onInput={(e) => setBio(e.currentTarget.value)} maxLength={4096} rows={3} />
        </Field>
        <Checkbox checked={wallet()} onChange={setWallet} label={t("people_disclose_wallet")} hint="They can pay you. Leave it off and they see only your username." />
        <div class="flex justify-end">
          <Button
            loading={busy()}
            onClick={async () => {
              setBusy(true);
              try {
                await ipc.peopleSendCard(props.address, bio(), wallet());
                store.toast("Card sent");
                props.onClose();
              } catch (e) {
                store.toast(errText(e), "error");
              } finally {
                setBusy(false);
              }
            }}
          >
            {t("send")}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

export function AuthorFeed(props: { address: string }) {
  const [items] = createResource(
    () => ({ a: props.address, tick: store.ticks().feed }),
    (k) => ipc.feedAuthor(k.a, 0, 30).catch(() => []),
  );
  return (
    <Show when={(items() ?? []).length} fallback={<p class="text-xs text-muted">{items.loading ? t("loading") : "No public posts."}</p>}>
      <ul class="card divide-y divide-border">
        <For each={items() ?? []}>
          {(it) => (
            <li class="px-3 py-2 text-[13px]">
              <span class="text-[11px] text-muted">{formatMs(it.timestamp * 1000)}</span>
              <p class="selectable whitespace-pre-wrap">{String((it.payload as { text?: string }).text ?? (it.kind === "REPOST" ? "reposted" : ""))}</p>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

export function MyProfileDialog(props: { open: boolean; onClose: () => void }) {
  const me = () => store.status()?.address ?? "";
  const [name, setName] = createSignal("");
  const [bio, setBio] = createSignal("");
  const [avatar, setAvatar] = createSignal<string>("");
  const [privName, setPrivName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  createEffect(() => {
    if (!props.open) return;
    void ipc.peopleProfile(me()).then((p) => {
      setName(p.display_name);
      setBio(p.bio);
    }).catch(() => undefined);
    void ipc.peopleMyDisplayName().then(setPrivName).catch(() => undefined);
  });
  return (
    <Dialog open={props.open} onClose={props.onClose} title="My profile" width="max-w-md">
      <div class="flex flex-col gap-4">
        <div class="flex items-center gap-3">
          <Avatar address={me()} size={40} />
          <div class="min-w-0">
            <Mono text={me()} head={14} tail={8} copy />
            <p class="text-xs text-muted">Your address is public by construction.</p>
          </div>
        </div>
        <div class="card p-3">
          <p class="mb-2 text-[11px] font-medium uppercase tracking-wide text-muted">Public profile (everyone on the network)</p>
          <Field label="Display name">
            <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={128} />
          </Field>
          <Field label="Bio" class="mt-2">
            <Textarea value={bio()} onInput={(e) => setBio(e.currentTarget.value)} maxLength={4096} rows={3} />
          </Field>
          <div class="mt-2 flex items-center gap-2 text-xs">
            <Button variant="secondary" size="sm" onClick={async () => { const f = await pickFile({ filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp"] }] }); if (f[0]) setAvatar(f[0]); }}>
              Choose avatar…
            </Button>
            <span class="truncate text-muted">{avatar() || "no change"}</span>
          </div>
          <div class="mt-3 flex justify-end">
            <Button
              size="sm"
              loading={busy()}
              onClick={async () => {
                setBusy(true);
                try {
                  await ipc.feedProfileUpdate(name(), bio(), avatar() || undefined);
                  store.toast("Public profile published");
                  if (avatar()) { forgetAvatar(me()); setAvatar(""); }
                  store.bump("people", "feed");
                } catch (e) {
                  store.toast(errText(e), "error");
                } finally {
                  setBusy(false);
                }
              }}
            >
              Publish
            </Button>
          </div>
        </div>
        <div class="card p-3">
          <p class="mb-2 text-[11px] font-medium uppercase tracking-wide text-muted">Private name (mail headers and cards only)</p>
          <div class="flex items-center gap-2">
            <Input value={privName()} onInput={(e) => setPrivName(e.currentTarget.value)} maxLength={128} placeholder="How your mail signs you" />
            <Button
              size="sm"
              variant="secondary"
              onClick={async () => {
                try {
                  await ipc.peopleSetDisplayName(privName());
                  store.toast("Saved");
                } catch (e) {
                  store.toast(errText(e), "error");
                }
              }}
            >
              {t("save")}
            </Button>
          </div>
        </div>
        <Button variant="ghost" size="sm" onClick={() => void copyText(`hashgram://user/${me()}`)}>
          <CopyIcon size={12} /> Copy my hashgram:// link
        </Button>
      </div>
    </Dialog>
  );
}
