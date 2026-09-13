// My profile: who I am on the network, what I hold and what I have done —
// avatar, name, address, HASH balance, activity score, counts, the walls I
// opened and wrote on, and a complete list of everything I published.
import { For, Show, createResource, createSignal } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { Pencil, RefreshCw, Wallet, Megaphone, ListChecks, Heart, MessageSquare, Repeat2, FileText, UserPlus, Trash2, UserCircle } from "lucide-solid";
import { Button, Badge, Empty, Stat } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { PersonAvatar, Mono, forgetAvatar } from "~/components/identity";
import { ipc, type FeedItem } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatMs, shortWhen } from "~/lib/format";
import { MyProfileDialog } from "./People";
import { WallCard, WallChip, postText } from "./feed/Feed";

const KIND_LABEL: Record<string, string> = {
  POST_CREATE: "posted",
  POST_DELETE: "deleted a post",
  COMMENT: "commented",
  REACTION: "reacted",
  REPOST: "reposted",
  REEL_CREATE: "posted a reel",
  STORY_CREATE: "posted a story",
  PROFILE_UPDATE: "updated profile",
  FOLLOW: "followed",
  UNFOLLOW: "unfollowed",
  CHANNEL_CREATE: "opened a wall",
};

function KindIcon(props: { kind: string }) {
  switch (props.kind) {
    case "COMMENT": return <MessageSquare size={12} />;
    case "REACTION": return <Heart size={12} />;
    case "REPOST": return <Repeat2 size={12} />;
    case "FOLLOW": case "UNFOLLOW": return <UserPlus size={12} />;
    case "POST_DELETE": return <Trash2 size={12} />;
    case "CHANNEL_CREATE": return <Megaphone size={12} />;
    case "PROFILE_UPDATE": return <UserCircle size={12} />;
    default: return <FileText size={12} />;
  }
}

export function MeRoute() {
  const navigate = useNavigate();
  const [edit, setEdit] = createSignal(false);
  const [showAll, setShowAll] = createSignal(false);
  const [me, { refetch }] = createResource(
    () => ({ tick: store.ticks().feed, people: store.ticks().people, wallet: store.ticks().wallet }),
    () => ipc.profileMe(),
  );
  const [events, { refetch: refetchEvents }] = createResource(
    () => (showAll() ? store.ticks().feed : null),
    () => ipc.profileMyEvents(0, 500),
  );
  const address = () => me()?.address ?? store.status()?.address ?? "";
  const score = () => me()?.activity.score ?? 0;
  const target = (it: FeedItem): string => {
    const p = it.payload as { post_id?: string; target?: string; address?: string; target_id?: string };
    return String(p.post_id ?? p.target ?? p.target_id ?? p.address ?? "");
  };

  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="min-h-0 flex-1 overflow-auto">
        <div class="mx-auto max-w-2xl px-4 py-4">
          <Show when={!me.error} fallback={<ErrorState error={me.error} onRetry={() => void refetch()} />}>
            <Show when={me()} fallback={<div class="flex flex-col gap-2"><div class="skeleton h-24 w-full" /><div class="skeleton h-20 w-full" /></div>}>
              {(p) => (
                <>
                  <header class="card flex items-start gap-4 p-4" data-testid="me-header">
                    <PersonAvatar address={p().address} size={64} />
                    <div class="min-w-0 flex-1">
                      <h1 class="truncate text-lg font-semibold">{p().display_name || (p().username ? `@${p().username}` : "Unnamed")}</h1>
                      <p class="flex flex-wrap items-center gap-x-3 text-xs text-muted">
                        <Show when={p().username}><span>@{p().username}</span></Show>
                        <Mono text={p().address} head={14} tail={8} copy />
                      </p>
                      <Show when={p().bio}>
                        <p class="mt-2 whitespace-pre-wrap text-[13px] selectable">{p().bio}</p>
                      </Show>
                      <Show when={!p().refreshed}>
                        <p class="mt-1 text-[11px] text-muted">Showing what this PC holds; the network was not reachable to refresh.</p>
                      </Show>
                    </div>
                    <div class="flex flex-col gap-1">
                      <Button size="sm" onClick={() => setEdit(true)}>
                        <Pencil size={12} /> {t("me_edit")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => { forgetAvatar(address()); void refetch(); }} title="Refresh from the network">
                        <RefreshCw size={12} /> Refresh
                      </Button>
                    </div>
                  </header>

                  <section class="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-3" data-testid="me-stats">
                    <Stat
                      label={t("me_balance")}
                      value={<span class="tnum">{p().balance ? p().balance!.display : "—"}</span>}
                      sub={p().balance ? <button type="button" class="hover:underline" onClick={() => navigate("/wallet")}><Wallet size={10} class="mr-1 inline" />open wallet</button> : "balance unavailable offline"}
                      title={p().balance ? `${p().balance!.uhash} uhash` : ""}
                    />
                    <Stat label={t("me_score")} value={<span class="tnum">{score().toLocaleString()}</span>} sub="posts ×10 · walls ×15 · comments ×3 · received ♥ ×2, comments ×4" />
                    <Stat label={t("me_posts")} value={<span class="tnum">{p().activity.posts}</span>} sub={`${p().activity.reposts} reposts`} />
                    <Stat label={t("me_comments")} value={<span class="tnum">{p().activity.comments}</span>} />
                    <Stat label={t("me_reactions")} value={<span class="tnum">{p().activity.reactions}</span>} sub="given" />
                    <Stat label={t("me_received")} value={<span class="tnum">{p().activity.reactions_received} ♥ · {p().activity.comments_received} 💬</span>} />
                    <Stat label={t("me_walls")} value={<span class="tnum">{p().activity.walls_created}</span>} sub={`wrote on ${p().activity.walls_posted.length}`} />
                    <Stat label={t("me_following")} value={<span class="tnum">{p().following}</span>} />
                    <Stat label={t("me_friends")} value={<span class="tnum">{p().friends}</span>} />
                  </section>
                  <Show when={p().activity.first_event}>
                    <p class="mt-1 text-[11px] text-muted">
                      {p().activity.events} signed events on the network · first {formatMs(p().activity.first_event * 1000)} · last {formatMs(p().activity.last_event * 1000)}
                    </p>
                  </Show>

                  <section class="mt-4">
                    <h2 class="mb-2 flex items-center gap-2 text-sm font-medium">
                      <Megaphone size={14} /> My walls
                      <Button variant="ghost" size="sm" class="ml-auto" onClick={() => navigate("/hashwall/walls")}>Manage</Button>
                    </h2>
                    <Show when={p().walls.length} fallback={<p class="text-xs text-muted">You have not pinned or opened a wall yet.</p>}>
                      <For each={p().walls}>{(w) => <WallCard w={w} onOpen={() => navigate(`/hashwall/walls/${w.id}`)} />}</For>
                    </Show>
                    <Show when={p().activity.walls_posted.length}>
                      <p class="mt-1 flex flex-wrap items-center gap-1 text-xs text-muted">
                        Wrote on:
                        <For each={p().activity.walls_posted}>{(id) => <WallChip id={id} />}</For>
                      </p>
                    </Show>
                  </section>

                  <section class="mt-4">
                    <h2 class="mb-2 flex items-center gap-2 text-sm font-medium">
                      <ListChecks size={14} /> {t("me_everything")}
                      <Show when={showAll()}>
                        <Button variant="ghost" size="icon-sm" class="ml-auto" title="Refresh" onClick={() => void refetchEvents()}>
                          <RefreshCw size={12} />
                        </Button>
                      </Show>
                    </h2>
                    <Show when={showAll()} fallback={<Button variant="secondary" size="sm" onClick={() => setShowAll(true)}>Show everything I published</Button>}>
                      <Show when={!events.error} fallback={<ErrorState error={events.error} onRetry={() => void refetchEvents()} />}>
                        <Show when={events()} fallback={<div class="skeleton h-9 w-full" />}>
                          {(list) => (
                            <Show when={list().length} fallback={<Empty title="Nothing yet">Your signed posts, comments and reactions will be listed here.</Empty>}>
                              <ul class="card divide-y divide-border">
                                <For each={list()}>
                                  {(it) => (
                                    <li class="flex items-start gap-2 px-3 py-2 text-[13px]" data-event={it.id}>
                                      <span class="mt-0.5 text-muted"><KindIcon kind={it.kind} /></span>
                                      <span class="min-w-0 flex-1">
                                        <span class="flex flex-wrap items-center gap-2">
                                          <Badge>{KIND_LABEL[it.kind] ?? it.kind.toLowerCase()}</Badge>
                                          <Show when={it.channel}><WallChip id={it.channel} /></Show>
                                          <span class="tnum text-[11px] text-muted" title={formatMs(it.timestamp * 1000)}>{shortWhen(it.timestamp * 1000)}</span>
                                        </span>
                                        <Show when={postText(it)}>
                                          <button type="button" class="mt-0.5 block max-w-full truncate text-left hover:underline" onClick={() => navigate(`/hashwall/post/${it.kind === "COMMENT" ? target(it) || it.id : it.id}`)}>
                                            {postText(it)}
                                          </button>
                                        </Show>
                                        <Show when={!postText(it) && target(it)}>
                                          <Show when={target(it).startsWith("hash1")} fallback={<button type="button" class="mt-0.5 block text-left text-xs text-muted hover:underline" onClick={() => navigate(`/hashwall/post/${target(it)}`)}>on post {target(it).slice(0, 12)}…</button>}>
                                            <button type="button" class="mt-0.5 block text-left text-xs text-muted hover:underline" onClick={() => navigate(`/people/${target(it)}`)}><Mono text={target(it)} head={12} tail={6} /></button>
                                          </Show>
                                        </Show>
                                      </span>
                                    </li>
                                  )}
                                </For>
                              </ul>
                            </Show>
                          )}
                        </Show>
                      </Show>
                    </Show>
                  </section>
                </>
              )}
            </Show>
          </Show>
        </div>
      </div>
      <MyProfileDialog open={edit()} onClose={() => { setEdit(false); void refetch(); }} />
    </div>
  );
}
