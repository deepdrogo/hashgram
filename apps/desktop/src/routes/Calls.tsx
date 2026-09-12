// Calls. Discovery and TURN credentials come from the SDK (call nodes'
// signed announcements); offer/answer/ICE travel inside the MLS chat as
// CallSignal messages, so only the two members learn a call is happening.
// Media is WebRTC (the webview's stack): DTLS-SRTP end to end for 1:1.
// Group calls need an SFU node announced on the network; when none is, the
// button is disabled and says so — and an SFU is never E2EE against its
// operator, which the UI states before joining.
import { createResource, createSignal, onCleanup, onMount, Show, For } from "solid-js";
import { Phone, PhoneOff, Mic, MicOff, Video, VideoOff, Monitor } from "lucide-solid";
import { Button, Card, Notice, Skeleton, Empty } from "~/components/ui";
import { PersonLabel } from "~/components/identity";
import { ipc, type IceServer, type MessageView } from "~/lib/ipc";
import { store } from "~/lib/store";

function randomId(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

/** The call panel: a 1:1 WebRTC call over TURN, signalled through MLS. */
export function CallPanel(props: { groupId: string; peer: string; video: boolean; onClose: () => void; incoming?: MessageView }) {
  const [state, setState] = createSignal<string>("preparing");
  const [muted, setMuted] = createSignal(false);
  const [camOff, setCamOff] = createSignal(!props.video);
  const [err, setErr] = createSignal<string | null>(null);
  let localVideo!: HTMLVideoElement;
  let remoteVideo!: HTMLVideoElement;
  let pc: RTCPeerConnection | null = null;
  let local: MediaStream | null = null;
  let callId = props.incoming?.call?.call_id ?? randomId();
  let since = Date.now() - 60_000;
  let poll: ReturnType<typeof setInterval> | null = null;
  const seen = new Set<string>();

  const signal = (kind: string, opts: Parameters<typeof ipc.callsSignal>[3] = {}) => ipc.callsSignal(props.groupId, kind, callId, opts).catch((e) => store.toast(String(e), "error"));

  const setup = async (ice: IceServer) => {
    pc = new RTCPeerConnection({ iceServers: [{ urls: ice.urls, username: ice.username, credential: ice.credential }], iceTransportPolicy: "all" });
    pc.onicecandidate = (e) => {
      if (e.candidate) void signal("ice", { candidate: e.candidate.candidate, sdpMid: e.candidate.sdpMid ?? "", sdpMlineIndex: e.candidate.sdpMLineIndex ?? 0 });
    };
    pc.onconnectionstatechange = () => setState(pc?.connectionState ?? "closed");
    pc.ontrack = (e) => {
      if (remoteVideo && e.streams[0]) remoteVideo.srcObject = e.streams[0];
    };
    local = await navigator.mediaDevices.getUserMedia({ audio: true, video: props.video });
    local.getTracks().forEach((t) => pc!.addTrack(t, local!));
    if (localVideo) localVideo.srcObject = local;
  };

  const handle = async (m: MessageView) => {
    const s = m.call;
    if (!s || !pc || seen.has(m.id) || m.outgoing) return;
    if (s.call_id !== callId) return;
    seen.add(m.id);
    switch (s.kind) {
      case "answer":
        await pc.setRemoteDescription({ type: "answer", sdp: s.sdp });
        break;
      case "offer":
        await pc.setRemoteDescription({ type: "offer", sdp: s.sdp });
        {
          const ans = await pc.createAnswer();
          await pc.setLocalDescription(ans);
          await signal("answer", { sdp: ans.sdp ?? "" });
        }
        break;
      case "ice":
        try {
          await pc.addIceCandidate({ candidate: s.candidate, sdpMid: s.sdp_mid || null, sdpMLineIndex: s.sdp_mline_index });
        } catch {
          /* late candidates are fine */
        }
        break;
      case "hangup":
      case "busy":
        setState(s.kind);
        cleanup(false);
        break;
      default:
        break;
    }
  };

  const cleanup = (send: boolean) => {
    if (poll) clearInterval(poll);
    poll = null;
    if (send) void signal("hangup");
    local?.getTracks().forEach((t) => t.stop());
    pc?.close();
    pc = null;
  };

  onMount(async () => {
    try {
      setState("getting TURN credentials");
      const ice = await ipc.callsTurn();
      setState("starting media");
      await setup(ice);
      if (props.incoming) {
        setState("answering");
        await handle(props.incoming);
      } else {
        await signal("ring", { video: props.video });
        const offer = await pc!.createOffer();
        await pc!.setLocalDescription(offer);
        await signal("offer", { sdp: offer.sdp ?? "", video: props.video });
        setState("ringing");
      }
      poll = setInterval(async () => {
        const sigs = await ipc.callsSignals(props.groupId, since).catch(() => [] as MessageView[]);
        for (const m of sigs) {
          await handle(m);
          since = Math.max(since, m.timestamp_ms - 1);
        }
      }, 1500);
    } catch (e) {
      setErr(String(e));
      setState("failed");
    }
  });
  onCleanup(() => cleanup(false));

  const toggleMute = () => {
    local?.getAudioTracks().forEach((t) => (t.enabled = muted()));
    setMuted((v) => !v);
  };
  const toggleCam = () => {
    local?.getVideoTracks().forEach((t) => (t.enabled = camOff()));
    setCamOff((v) => !v);
  };
  const share = async () => {
    try {
      const ds = await navigator.mediaDevices.getDisplayMedia({ video: true });
      const track = ds.getVideoTracks()[0];
      const sender = pc?.getSenders().find((s) => s.track?.kind === "video");
      if (sender && track) await sender.replaceTrack(track);
      else if (track && pc) pc.addTrack(track, ds);
    } catch (e) {
      store.toast(String(e), "error");
    }
  };

  return (
    <div class="fixed inset-0 z-50 flex flex-col bg-bg" role="dialog" aria-label="Call">
      <header class="flex items-center gap-3 border-b border-border px-4 py-2">
        <PersonLabel person={{ address: props.peer }} />
        <span class="text-xs text-muted">· {state()}</span>
        <span class="flex-1" />
        <span class="mono text-[10px] text-muted">E2EE (DTLS-SRTP) · TURN relays ciphertext only</span>
      </header>
      <div class="relative min-h-0 flex-1 bg-bg">
        <video ref={remoteVideo} autoplay playsinline class="h-full w-full object-contain" />
        <video ref={localVideo} autoplay muted playsinline class="absolute bottom-4 right-4 h-36 w-48 rounded-md border border-border object-cover" />
        <Show when={err()}>
          <div class="absolute inset-x-0 top-4 mx-auto max-w-md">
            <Notice strong title="Call could not start">{err()}</Notice>
          </div>
        </Show>
      </div>
      <footer class="flex items-center justify-center gap-3 border-t border-border py-3">
        <Button size="icon" variant="secondary" onClick={toggleMute} title={muted() ? "Unmute" : "Mute"}>
          {muted() ? <MicOff size={16} /> : <Mic size={16} />}
        </Button>
        <Button size="icon" variant="secondary" onClick={toggleCam} title={camOff() ? "Camera on" : "Camera off"}>
          {camOff() ? <VideoOff size={16} /> : <Video size={16} />}
        </Button>
        <Button size="icon" variant="secondary" onClick={share} title="Share screen">
          <Monitor size={16} />
        </Button>
        <Button size="icon" onClick={() => { cleanup(true); props.onClose(); }} title="Hang up">
          <PhoneOff size={16} />
        </Button>
      </footer>
    </div>
  );
}

/** The Calls page: infrastructure on the network and incoming rings. */
export function Calls() {
  const [infra, { refetch }] = createResource(() => ipc.callsDiscover().catch((e) => ({ error: String(e) })));
  const [incoming, setIncoming] = createSignal<{ m: MessageView; peer: string } | null>(null);
  const [active, setActive] = createSignal<{ groupId: string; peer: string; video: boolean; incoming?: MessageView } | null>(null);
  const me = () => store.status()?.address ?? "";

  // Poll recent ring/offer signals across conversations to surface incoming calls.
  onMount(() => {
    let since = Date.now() - 30_000;
    const t = setInterval(async () => {
      const convs = await ipc.chatList().catch(() => []);
      for (const c of convs) {
        if (!c.direct) continue;
        const sigs = await ipc.callsSignals(c.group_id, since).catch(() => []);
        for (const m of sigs) {
          if (!m.outgoing && m.call?.kind === "offer" && !active()) {
            setIncoming({ m, peer: c.members.find((x) => x !== me()) ?? m.sender });
          }
          since = Math.max(since, m.timestamp_ms - 1);
        }
      }
    }, 3000);
    onCleanup(() => clearInterval(t));
  });

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <h1 class="page-title">Calls</h1>
        <Button size="sm" variant="secondary" onClick={() => void refetch()}>Rediscover</Button>
      </div>
      <Show when={incoming()}>
        {(inc) => (
          <Card class="border-fg p-4">
            <div class="flex items-center gap-3">
              <Phone size={18} />
              <span class="text-sm">Incoming {inc().m.call?.video ? "video" : "voice"} call from</span>
              <PersonLabel person={{ address: inc().peer }} />
              <span class="flex-1" />
              <Button onClick={() => { setActive({ groupId: inc().m.group_id, peer: inc().peer, video: !!inc().m.call?.video, incoming: inc().m }); setIncoming(null); }}>Answer</Button>
              <Button variant="secondary" onClick={() => { void ipc.callsSignal(inc().m.group_id, "busy", inc().m.call!.call_id); setIncoming(null); }}>Decline</Button>
            </div>
          </Card>
        )}
      </Show>
      <Show when={infra()} fallback={<Skeleton lines={3} />}>
        {(i) => (
          <Show when={!("error" in i())} fallback={<Notice strong title="Call infrastructure unavailable">{(i() as { error: string }).error}</Notice>}>
            {(() => {
              const v = i() as Exclude<typeof i extends () => infer T ? T : never, { error: string }>;
              return (
                <>
                  <div class="grid grid-cols-2 gap-3">
                    <Card title="TURN nodes announced">
                      <Show when={v.nodes.length} fallback={<Empty title="No call nodes announced">Nobody on the network is offering TURN right now. Calls need at least one node with the call role.</Empty>}>
                        <ul>
                          <For each={v.nodes}>
                            {(n) => (
                              <li class="border-b border-border px-4 py-2 text-xs last:border-0">
                                <p class="mono">{String((n as { turn_uris?: string[] }).turn_uris?.join(", ") ?? "")}</p>
                                <p class="text-muted">operator {String((n as { operator?: string }).operator ?? "—")} · realm {String((n as { realm?: string }).realm ?? "")} · {(n as { issues_credentials?: boolean }).issues_credentials ? "issues credentials" : "no credentials"}</p>
                              </li>
                            )}
                          </For>
                        </ul>
                      </Show>
                    </Card>
                    <Card title="Group calls">
                      <div class="p-4">
                        <Button disabled={!v.sfu_available} title={v.sfu_available ? "" : "no SFU node available"}>
                          Start group call
                        </Button>
                        <p class="mt-2 text-xs text-muted">
                          {v.sfu_available
                            ? "An SFU is announced. Group calls through an SFU are NOT end-to-end encrypted against the SFU operator."
                            : "No SFU node available on the network, so group calls are disabled. When one appears, group calls will still not be end-to-end encrypted against the SFU operator — the app will say so before you join."}
                        </p>
                      </div>
                    </Card>
                  </div>
                  <Notice>Start a 1:1 call from a direct conversation (the phone icon). Signalling travels inside the encrypted chat; media is WebRTC with DTLS-SRTP.</Notice>
                </>
              );
            })()}
          </Show>
        )}
      </Show>
      <Show when={active()}>
        <CallPanel groupId={active()!.groupId} peer={active()!.peer} video={active()!.video} incoming={active()!.incoming} onClose={() => setActive(null)} />
      </Show>
    </div>
  );
}
