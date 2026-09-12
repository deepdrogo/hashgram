// The Connected-nodes panel in full: peers with roles, operator, latency,
// transport, discovery layer, who served the last read and whether they
// agreed; wrong-network peers greyed with the reason; own NAT status;
// chain source precedence with health dots; genesis pin; seeds; peerstore;
// forget peers; diagnostics export with no IPs of other peers.
import { createResource, createSignal, For, Show } from "solid-js";
import { save } from "@tauri-apps/plugin-dialog";
import { Card, Button, Notice, Skeleton, Badge, Stat } from "~/components/ui";
import { Mono, HealthDot } from "~/components/identity";
import { ipc, pick, str } from "~/lib/ipc";
import { store } from "~/lib/store";
import { formatDuration, sourceLabel, verificationLabel } from "~/lib/format";
import { useChain } from "~/lib/chain";

export function Network() {
  const net = store.net;
  const [health, { refetch: reprobe }] = createResource(() => ipc.chainHealth(false).catch(() => null));
  const [info] = useChain(() => "hashgram/network/v1/info");
  const [fork] = useChain(() => "hashgram/network/v1/fork_isolation");
  const [latest] = useChain(() => "cosmos/base/tendermint/v1beta1/blocks/latest");
  const [busy, setBusy] = createSignal<string | null>(null);

  const act = async (name: string, f: () => Promise<unknown>) => {
    setBusy(name);
    try {
      await f();
      await store.refreshNet();
      await reprobe();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const exportDiag = () =>
    act("diag", async () => {
      const text = await ipc.diagnosticsExport();
      const path = await save({ defaultPath: "hashgram-diagnostics.txt", filters: [{ name: "Text", extensions: ["txt"] }] });
      if (path) {
        await ipc.saveTextFile(path, text);
        store.toast("Diagnostics exported (no peer IP addresses included)");
      }
    });
  const height = () => str(pick(latest()?.ok ? (latest() as { value: unknown }).value : null, "block.header.height"), "—");
  const blockTime = () => str(pick(latest()?.ok ? (latest() as { value: unknown }).value : null, "block.header.time"), "");
  const genesisPinned = () => net()?.genesis_hash === "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d";

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <h1 class="page-title">Network</h1>
        <div class="flex gap-2">
          <Button size="sm" variant="secondary" loading={busy() === "latency"} onClick={() => act("latency", () => ipc.netMeasureLatency())}>
            Measure latency
          </Button>
          <Button size="sm" variant="secondary" loading={busy() === "probe"} onClick={() => act("probe", () => ipc.chainHealth(true))}>
            Re-check sources
          </Button>
          <Button size="sm" variant="secondary" loading={busy() === "diag"} onClick={exportDiag}>
            Export diagnostics
          </Button>
        </div>
      </div>

      <div class="grid grid-cols-4 gap-3">
        <Stat label="Connected nodes" value={`${net()?.verified ?? 0}`} sub={`${net()?.peers.length ?? 0} connections · ${net()?.kad_peers ?? 0} in DHT`} />
        <Stat label="Chain height" value={height()} sub={blockTime() ? new Date(blockTime()).toLocaleTimeString() : "from relayed reads"} />
        <Stat label="Genesis pin" mono={false} value={<span class="flex items-center gap-2">{genesisPinned() ? "✓ Mainnet" : net()?.network === "devnet" ? "DEVNET" : "—"}</span>} sub={<Mono text={net()?.genesis_hash ?? ""} head={12} tail={8} copy class="text-xs" />} />
        <Stat label="Your NAT" mono={false} value={net()?.nat ?? "unknown"} sub={`peer id ${net()?.own_peer_id.slice(0, 12) ?? ""}… (ephemeral)`} />
      </div>

      <Card title="Chain sources (precedence order)">
        <Show when={health()} fallback={<div class="p-4"><Skeleton lines={3} /></div>}>
          <ul>
            <For each={health()!.sources}>
              {(s) => (
                <li class="flex items-center gap-3 border-b border-border px-4 py-2 text-sm last:border-0">
                  <HealthDot state={s.ok ? "ok" : s.detail.includes("wrong network") ? "bad" : "warn"} title={s.detail} />
                  <span class="w-40 shrink-0">{sourceLabel(s.source)}</span>
                  <span class="min-w-0 flex-1 truncate text-xs text-muted">{s.detail}</span>
                  <span class="mono text-xs text-muted">{s.latency_ms} ms</span>
                  <Show when={s.active}>
                    <Badge strong>active</Badge>
                  </Show>
                </li>
              )}
            </For>
          </ul>
          <p class="px-4 py-2 text-xs text-muted">
            Last read: {verificationLabel(net()?.last_read, health()?.active)} · relay operators reachable: {health()!.relay_operators}. A source on another chain id is "wrong network" and never used.
          </p>
        </Show>
      </Card>

      <Card title="Connected nodes">
        <Show when={net()} fallback={<div class="p-4"><Skeleton lines={3} /></div>}>
          <Show when={net()!.peers.length} fallback={<p class="p-4 text-xs text-muted">No connections yet. Allow outbound UDP and TCP 26670 in Windows Firewall if this persists.</p>}>
            <table class="table">
              <thead>
                <tr>
                  <th>Peer</th>
                  <th>Operator</th>
                  <th>Roles</th>
                  <th>Latency</th>
                  <th>Transport</th>
                  <th>Found via</th>
                  <th>Last read</th>
                </tr>
              </thead>
              <tbody>
                <For each={net()!.peers}>
                  {(p) => (
                    <tr class={p.verified ? "" : "opacity-50"}>
                      <td>
                        <Mono text={p.peer_id} head={10} tail={6} copy />
                        <Show when={!p.verified}>
                          <Badge class="ml-2">handshaking</Badge>
                        </Show>
                      </td>
                      <td>{p.operator ? <Mono text={p.operator} head={10} tail={4} /> : <span class="text-muted">—</span>}</td>
                      <td>
                        <span class="flex flex-wrap gap-1">
                          <For each={p.roles}>{(r) => <Badge strong={r === "relay" || r === "bootstrap"}>{r}</Badge>}</For>
                        </span>
                      </td>
                      <td class="mono">{p.latency_ms !== null ? `${p.latency_ms} ms` : "—"}</td>
                      <td>
                        {p.transport} <span class="text-xs text-muted">{p.direction}</span>
                      </td>
                      <td class="text-xs">{p.discovery}</td>
                      <td class="text-xs">
                        <Show when={p.served_last_read} fallback={p.agreed === false ? <Badge strong>disputed</Badge> : <span class="text-muted">—</span>}>
                          <Badge strong={p.agreed === true}>{p.agreed === true ? "served · agreed" : p.agreed === false ? "served · disputed" : "served"}</Badge>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </Show>
      </Card>

      <Show when={net()?.rejected.length}>
        <Card title="Rejected peers (never retried silently)">
          <ul>
            <For each={net()!.rejected}>
              {(r) => (
                <li class="flex items-center gap-3 border-b border-border px-4 py-2 text-sm opacity-60 last:border-0">
                  <Mono text={r.peer_id} head={10} tail={6} />
                  <Badge strong={r.label === "wrong network"}>{r.label}</Badge>
                  <span class="text-xs text-muted">{r.reason}</span>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>

      <div class="grid grid-cols-2 gap-3">
        <Card title="Protocol facts">
          <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 p-4 text-xs">
            <dt class="text-muted">Chain id</dt>
            <dd class="mono">{net()?.chain_id}</dd>
            <dt class="text-muted">Network</dt>
            <dd>{net()?.network}</dd>
            <Show when={info()?.ok}>
              <dt class="text-muted">Info</dt>
              <dd class="mono break-all">{JSON.stringify(pick((info() as { value: unknown }).value, "info") ?? (info() as { value: unknown }).value)}</dd>
            </Show>
            <Show when={fork()?.ok}>
              <dt class="text-muted">Fork isolation</dt>
              <dd class="mono break-all">{JSON.stringify((fork() as { value: unknown }).value)}</dd>
            </Show>
            <dt class="text-muted">Link uptime</dt>
            <dd>{formatDuration(net()?.uptime_secs ?? 0)}</dd>
          </dl>
        </Card>
        <Card title="Discovery">
          <div class="flex flex-col gap-3 p-4 text-xs">
            <div>
              <p class="mb-1 text-muted">Built-in seeds (compiled into this app)</p>
              <ul class="mono space-y-0.5">
                <For each={net()?.builtin_seeds ?? []}>{(s) => <li class="truncate">{s}</li>}</For>
              </ul>
            </div>
            <p class="text-muted">
              Peerstore: {net()?.peerstore_size ?? 0} remembered peer(s). After the first run the peerstore comes first; the built-in list is the fallback.
            </p>
            <div class="flex gap-2">
              <Button size="sm" variant="secondary" loading={busy() === "reconnect"} onClick={() => act("reconnect", () => ipc.netReconnect())}>
                Reconnect
              </Button>
              <Button size="sm" variant="ghost" loading={busy() === "forget"} onClick={() => act("forget", () => ipc.netForgetPeers())}>
                Forget peers
              </Button>
            </div>
          </div>
        </Card>
      </div>
      <Notice>No IP geolocation is computed or shown. The diagnostics export contains your own addresses and the peer ids you are connected to, never other peers' IP addresses.</Notice>
    </div>
  );
}
