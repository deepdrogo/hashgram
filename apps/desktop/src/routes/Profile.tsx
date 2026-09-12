// A person, by public key. Address, verified @username, identity and
// devices from chain; display name, bio and posts from their signed social
// events. Whatever the display name says, the verified handle sits beside it.
import { Show, For, createResource, createSignal } from "solid-js";
import { useParams, useNavigate } from "@solidjs/router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { UserPlus, UserMinus, MessageSquare, Ban, VolumeX, Pencil } from "lucide-solid";
import { Card, Notice, Skeleton, Badge, Button, Dialog, Field, Input, Textarea } from "~/components/ui";
import { PersonLabel, Mono, Avatar, VerifiedBy } from "~/components/identity";
import { useChain, readOf } from "~/lib/chain";
import { ipc, pick, str, arr, type EventView, type MediaView } from "~/lib/ipc";
import { isHashAddress, formatHash } from "~/lib/format";
import { store } from "~/lib/store";
import { forget } from "~/lib/people";
import { PostCard } from "./Feed";

export function Profile() {
  const params = useParams<{ address: string }>();
  const navigate = useNavigate();
  const address = () => params.address;
  const valid = () => isHashAddress(address());
  const me = () => store.status()?.address ?? "";
  const [reverse] = useChain(() => (valid() ? `hashgram/username/v1/reverse/${address()}` : null));
  const [identity] = useChain(() => (valid() ? `hashgram/identity/v1/identity/${address()}` : null));
  const [devices] = useChain(() => (valid() ? `hashgram/identity/v1/devices/${address()}` : null));
  const [balance] = useChain(() => (valid() ? `cosmos/bank/v1beta1/balances/${address()}/by_denom?denom=uhash` : null));
  const [prof, { refetch: refetchProf }] = createResource(address, (a) => ipc.profileGet(a, true).catch(() => null));
  const [posts, { refetch: refetchPosts }] = createResource(address, () => ipc.feed(["POST_CREATE", "REEL_CREATE"], undefined, undefined, 100).then((p) => p.events.filter((e) => e.author === address())).catch(() => [] as EventView[]));
  const [edit, setEdit] = createSignal(false);
  const username = () => (reverse()?.ok ? str(pick((reverse() as { value: unknown }).value, "name")) || null : null);
  const displayName = () => str(pick(prof()?.profile, "display_name")) || null;
  const bio = () => str(pick(prof()?.profile, "bio"));
  const found = () => identity()?.ok && pick((identity() as { value: unknown }).value, "found") === true;
  const deviceList = () => (devices()?.ok ? arr(pick((devices() as { value: unknown }).value, "devices")) : []);

  const toggleFollow = async () => {
    try {
      await ipc.follow(address(), !prof()?.following);
      await refetchProf();
      store.toast(prof()?.following ? "Following" : "Unfollowed");
    } catch (e) {
      store.toast(String(e), "error");
    }
  };
  const setBlock = async (mode: string | null) => {
    await ipc.socialBlock(address(), mode).catch((e) => store.toast(String(e), "error"));
    await refetchProf();
  };
  const message = async () => {
    try {
      const g = await ipc.chatStartDirect(address());
      navigate(`/messages/${g}`);
    } catch (e) {
      store.toast(String(e), "error");
    }
  };

  return (
    <div class="page flex flex-col gap-4">
      <Show when={valid()} fallback={<Notice strong>Not a Hashgram address.</Notice>}>
        <div class="flex items-start gap-4">
          <Avatar address={address()} size={64} />
          <div class="min-w-0 flex-1">
            <PersonLabel person={{ address: address(), username: username(), displayName: displayName() }} size="lg" copy />
            <div class="mt-1">
              <Mono text={address()} full copy class="text-xs text-muted" />
            </div>
            <Show when={bio()}>
              <p class="selectable mt-2 max-w-xl text-sm">{bio()}</p>
            </Show>
            <p class="mt-1 text-xs text-muted">{prof()?.events ?? 0} verified events cached · display names are self-declared; the handle above is verified on chain</p>
          </div>
          <div class="flex flex-wrap justify-end gap-2">
            <Show when={address() !== me()} fallback={<Button variant="secondary" onClick={() => setEdit(true)}><Pencil size={12} /> Edit profile</Button>}>
              <Button variant={prof()?.following ? "secondary" : "primary"} onClick={toggleFollow}>
                {prof()?.following ? <><UserMinus size={12} /> Unfollow</> : <><UserPlus size={12} /> Follow</>}
              </Button>
              <Button variant="secondary" onClick={message}>
                <MessageSquare size={12} /> Message
              </Button>
              <Button variant="secondary" onClick={() => navigate(`/wallet/send?to=${address()}`)}>
                Send HASH
              </Button>
              <Button variant="ghost" title="Mute (local)" onClick={() => void setBlock(prof()?.block_mode === "mute" ? null : "mute")}>
                <VolumeX size={12} /> {prof()?.block_mode === "mute" ? "Unmute" : "Mute"}
              </Button>
              <Button variant="ghost" title="Block (local)" onClick={() => void setBlock(prof()?.block_mode === "block" ? null : "block")}>
                <Ban size={12} /> {prof()?.block_mode === "block" ? "Unblock" : "Block"}
              </Button>
            </Show>
          </div>
        </div>
        <div class="grid grid-cols-3 gap-3">
          <Card title="Identity">
            <div class="p-4 text-sm">
              <Show when={!identity.loading} fallback={<Skeleton lines={2} />}>
                <Show when={found()} fallback={<p class="text-muted">No identity registered on chain for this address. Their posts and messages cannot be verified until there is one.</p>}>
                  <p>Registered · root rotations {str(pick((identity() as { value: unknown }).value, "identity.rotation_count"), "0")}</p>
                </Show>
              </Show>
            </div>
          </Card>
          <Card title="Devices">
            <Show when={deviceList().length} fallback={<p class="p-4 text-sm text-muted">None.</p>}>
              <ul class="p-2">
                <For each={deviceList()}>
                  {(d) => (
                    <li class="flex items-center justify-between px-2 py-1 text-sm">
                      <span>{str(pick(d, "label"), "(no label)")}</span>
                      <span class="flex items-center gap-2">
                        <Badge>{str(pick(d, "platform"))}</Badge>
                        <Show when={pick(d, "revoked") === true}>
                          <Badge strong>revoked</Badge>
                        </Show>
                      </span>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Card>
          <Card title="Balance">
            <div class="p-4">
              <Show when={balance()?.ok} fallback={<Skeleton />}>
                <p class="mono text-lg">{formatHash(str(pick((balance() as { value: unknown }).value, "balance.amount"), "0"))} HASH</p>
                <VerifiedBy verification={readOf(balance())?.verification} source={readOf(balance())?.source} />
              </Show>
            </div>
          </Card>
        </div>
        <h2 class="text-sm font-medium">Posts and reels</h2>
        <Show when={!posts.loading} fallback={<Skeleton lines={3} />}>
          <Show when={posts()?.length} fallback={<Card><div class="p-4 text-sm text-muted">{prof()?.following || address() === me() ? "Nothing published yet." : "Follow to fetch their posts (events are pulled for people you follow)."}</div></Card>}>
            <For each={posts()}>{(p) => <PostCard ev={p} onChanged={() => void refetchPosts()} />}</For>
          </Show>
        </Show>
      </Show>
      <EditProfile open={edit()} onClose={() => setEdit(false)} current={{ displayName: displayName() ?? "", bio: bio() }} onSaved={() => { forget(address()); void refetchProf(); }} />
    </div>
  );
}

function EditProfile(props: { open: boolean; onClose: () => void; current: { displayName: string; bio: string }; onSaved: () => void }) {
  const [name, setName] = createSignal(props.current.displayName);
  const [bio, setBio] = createSignal(props.current.bio);
  const [avatar, setAvatar] = createSignal<MediaView | null>(null);
  const [busy, setBusy] = createSignal(false);
  const pickAvatar = async () => {
    const path = await openDialog({ multiple: false, title: "Choose an avatar image" });
    if (!path || typeof path !== "string") return;
    setBusy(true);
    try {
      setAvatar(await ipc.mediaUpload(path));
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    try {
      await ipc.profileUpdate(name(), bio(), avatar() ?? undefined);
      props.onSaved();
      props.onClose();
      store.toast("Profile published");
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Edit profile" description="A signed social event. A display name is not identity: people always see your verified @username or address next to it." footer={<><Button variant="secondary" onClick={props.onClose}>Cancel</Button><Button onClick={save} loading={busy()}>Publish</Button></>}>
      <div class="flex flex-col gap-3">
        <Field label="Display name">
          <Input value={name()} onInput={(e) => setName(e.currentTarget.value)} maxLength={64} />
        </Field>
        <Field label="Bio">
          <Textarea rows={3} value={bio()} onInput={(e) => setBio(e.currentTarget.value)} maxLength={500} />
        </Field>
        <div class="flex items-center gap-2">
          <Button size="sm" variant="secondary" onClick={pickAvatar} loading={busy()}>Choose avatar</Button>
          <Show when={avatar()}>
            <Badge>uploaded {avatar()!.mime}</Badge>
          </Show>
        </div>
      </div>
    </Dialog>
  );
}

