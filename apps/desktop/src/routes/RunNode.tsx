// Earn → Run a node on this PC. Setup (roles, quota, bandwidth, cold reward
// address, moniker), install as a background task or service, live status
// from the node's local API and the chain, funding the bond, and honest
// expectations about NAT and demand.
import { createResource, createSignal, For, Show, onMount, onCleanup } from "solid-js";
import { Play, Square, Download, Trash2, RefreshCw, Wallet as WalletIcon } from "lucide-solid";
import { Button, Card, Field, Input, Notice, Skeleton, Stat, Switch, Badge, Dialog } from "~/components/ui";
import { Mono } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { ipc, pick, str, arr, num, coin, type NodeSetup, type NodeOverview, type MsgSpec } from "~/lib/ipc";
import { formatHash, isHashAddress, formatBytes, formatDuration } from "~/lib/format";
import { store } from "~/lib/store";

const ROLES: [string, string][] = [
  ["store", "keep mailboxes and blob replicas"],
  ["media", "serve blob chunks"],
  ["relay", "relay traffic and chain reads for wallets (no credit for chain reads)"],
];

export function RunNode() {
  const [ov, { refetch }] = createResource(() => ipc.nodeOverview().catch((e) => ({ error: String(e) })));
  const [busy, setBusy] = createSignal<string | null>(null);
  const [setup, setSetup] = createSignal<NodeSetup | null>(null);
  const [cold, setCold] = createSignal<{ words: string[]; address: string } | null>(null);
  const [fund, setFund] = createSignal<MsgSpec | null>(null);
  const [log, setLog] = createSignal<string | null>(null);

  onMount(() => {
    const t = setInterval(() => void refetch(), 10_000);
    onCleanup(() => clearInterval(t));
  });
  const o = () => (ov() && !("error" in ov()!) ? (ov() as NodeOverview) : null);
  const current = () => setup() ?? o()?.setup ?? { roles: ["store", "media", "relay"], storage_gib: 20, bandwidth_mbps: 0, reward_address: "", moniker: "home-pc", auto_register: true };
  const patch = (f: (s: NodeSetup) => void) => {
    const s = structuredClone(current());
    f(s);
    setSetup(s);
  };
  const act = async (name: string, f: () => Promise<unknown>, done?: string) => {
    setBusy(name);
    try {
      await f();
      if (done) store.toast(done);
      await refetch();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(null);
    }
  };
  const save = () => act("save", () => ipc.nodeConfigure(current()), "Node configuration written");
  const provider = () => o()?.provider && pick(o()!.provider, "provider");
  const bond = () => coin(pick(provider(), "bond"));
  const minBond = 1_000_000_000n; // 1,000 HASH
  const balance = () => BigInt(o()?.operator_balance_uhash ?? "0");
  const fundAmount = () => (minBond + 20_000_000n).toString(); // bond + ~20 HASH for fees

  return (
    <div class="flex flex-col gap-4">
      <Show when={ov()} fallback={<Skeleton lines={4} />}>
        <Show when={o()} fallback={<Notice strong>{(ov() as { error: string }).error}</Notice>}>
          {(n) => (
            <>
              <div class="grid grid-cols-4 gap-3">
                <Stat label="Node" mono={false} value={n().running ? "running" : n().configured ? "stopped" : "not set up"} sub={n().registration === "service" ? "Windows service" : n().registration === "scheduled_task" ? "background task (at logon)" : n().bundled ? "not installed" : "node binary not bundled in this build"} />
                <Stat label="Reachability" mono={false} value={n().reachability} sub={n().reachability === "private" ? "behind NAT: fewer assignments, less earned" : n().reachability === "public" ? "reachable from the internet" : "measured once peers connect"} />
                <Stat label="Operator" mono={false} value={n().operator ? <Mono text={n().operator!} head={10} tail={6} copy /> : "—"} sub={n().operator_balance_uhash !== null ? `${formatHash(n().operator_balance_uhash!)} HASH on the operator address` : "created with the configuration"} />
                <Stat label="Provider on chain" mono={false} value={provider() ? str(pick(provider(), "status"), "registered").replace("PROVIDER_STATUS_", "").toLowerCase() : "not registered"} sub={provider() ? `bond ${formatHash(bond(), 0, 0)} HASH` : "needs 1,000 HASH bond on the operator address"} />
              </div>

              <Notice strong title="What this does and does not promise">
                Your PC stores and serves other people's encrypted data and is paid from the service reserve for real bytes served, up to a per-epoch ceiling. Behind a NAT that hole punching cannot cross it receives fewer assignments and earns less. Earnings depend on demand. Nothing here is guaranteed.
              </Notice>

              <div class="grid grid-cols-2 gap-4">
                <Card title="Setup">
                  <div class="flex flex-col gap-3 p-4">
                    <div>
                      <p class="label">Roles</p>
                      <For each={ROLES}>
                        {([role, what]) => (
                          <Switch label={role} hint={what} checked={current().roles.includes(role)} onChange={(v) => patch((s) => (s.roles = v ? [...new Set([...s.roles, role])] : s.roles.filter((r) => r !== role)))} />
                        )}
                      </For>
                    </div>
                    <div class="grid grid-cols-2 gap-3">
                      <Field label="Disk quota (GiB)">
                        <Input mono type="number" min="1" value={current().storage_gib} onInput={(e) => patch((s) => (s.storage_gib = Number(e.currentTarget.value) || 1))} />
                      </Field>
                      <Field label="Bandwidth cap (Mbit/s, 0 = none)" hint="Announced to peers; advisory.">
                        <Input mono type="number" min="0" value={current().bandwidth_mbps} onInput={(e) => patch((s) => (s.bandwidth_mbps = Number(e.currentTarget.value) || 0))} />
                      </Field>
                    </div>
                    <Field label="Moniker">
                      <Input value={current().moniker} onInput={(e) => patch((s) => (s.moniker = e.currentTarget.value))} placeholder="home-pc" />
                    </Field>
                    <Field label="Reward address" hint="Where earnings are paid. Default: a separate cold address you write down, not your hot wallet." error={current().reward_address && !isHashAddress(current().reward_address) ? "not a hash1… address" : undefined}>
                      <div class="flex gap-2">
                        <Input mono value={current().reward_address} onInput={(e) => patch((s) => (s.reward_address = e.currentTarget.value.trim()))} placeholder="hash1…" />
                        <Button variant="secondary" onClick={() => void ipc.nodeGenerateColdAddress().then(setCold).catch((e) => store.toast(String(e), "error"))}>
                          Generate cold
                        </Button>
                        <Button variant="ghost" onClick={() => patch((s) => (s.reward_address = store.status()?.address ?? ""))} title="Use the hot wallet (not recommended)">
                          Hot wallet
                        </Button>
                      </div>
                    </Field>
                    <Switch label="Register as a provider automatically" hint="Once the operator address holds the 1,000 HASH bond plus fees, the node submits MsgRegisterProvider itself. The bond is slashed 5 % on proven fraud and unbonds over 21 days." checked={current().auto_register} onChange={(v) => patch((s) => (s.auto_register = v))} />
                    <div class="flex justify-end">
                      <Button onClick={save} loading={busy() === "save"} disabled={!current().roles.length || (!!current().reward_address && !isHashAddress(current().reward_address))}>
                        Write configuration
                      </Button>
                    </div>
                    <p class="text-xs text-muted">
                      The node reads the chain through this app's loopback gateway (<span class="mono">{n().chain_gateway}</span>), so no chain node is needed on this PC. Its operator key lives in your vault; a working copy is written to the node folder because the node signs receipts with it.
                    </p>
                  </div>
                </Card>

                <div class="flex flex-col gap-4">
                  <Card title="Run">
                    <div class="flex flex-col gap-3 p-4">
                      <Show when={!n().bundled}>
                        <Notice strong>This build does not include hashgram-node.exe and the service wrapper. The release script bundles them; in development they are picked up from the workspace target directory once built (cargo build -p hashgram-node -p hashgram-node-service).</Notice>
                      </Show>
                      <div class="flex flex-wrap gap-2">
                        <Button variant="secondary" disabled={!n().configured || !n().bundled} loading={busy() === "install"} onClick={() => act("install", () => ipc.nodeInstall(), n().elevated ? "Installed as a Windows service" : "Installed as a background task (runs at logon)")}>
                          <Download size={12} /> {n().registration === "none" ? "Install" : "Reinstall"}
                        </Button>
                        <Button disabled={n().registration === "none"} loading={busy() === "start"} onClick={() => act("start", () => ipc.nodeStart(), "Node starting")}>
                          <Play size={12} /> Start
                        </Button>
                        <Button variant="secondary" disabled={n().registration === "none"} loading={busy() === "stop"} onClick={() => act("stop", () => ipc.nodeStop(), "Node stopped")}>
                          <Square size={12} /> Stop
                        </Button>
                        <Button variant="ghost" disabled={n().registration === "none"} loading={busy() === "uninstall"} onClick={() => act("uninstall", () => ipc.nodeUninstall(), "Registration removed (data kept)")}>
                          <Trash2 size={12} /> Uninstall
                        </Button>
                        <Button variant="ghost" onClick={() => void ipc.nodeLogTail(200).then(setLog)}>
                          Log
                        </Button>
                        <Button variant="ghost" onClick={() => void refetch()}>
                          <RefreshCw size={12} />
                        </Button>
                      </div>
                      <p class="text-xs text-muted">
                        {n().elevated
                          ? "Running elevated: the node is registered as a Windows service (auto start, restart on failure)."
                          : "Not elevated (per-user install): the node is registered as a background task at logon, supervised and restarted by the service wrapper. Run Hashgram as administrator once to register a true Windows service instead."}
                      </p>
                      <Show when={n().binary}>
                        <p class="mono text-[10px] text-muted">{n().binary}</p>
                      </Show>
                    </div>
                  </Card>
                  <Card title="Bond">
                    <div class="flex flex-col gap-3 p-4 text-sm">
                      <Show when={n().operator} fallback={<p class="text-muted">Write the configuration first; the operator key is created then.</p>}>
                        <p>
                          Operator <Mono text={n().operator!} copy /> holds <span class="mono">{formatHash(balance())} HASH</span>. Registration needs 1,000 HASH bond plus fees.
                        </p>
                        <Show when={balance() < minBond}>
                          <Button onClick={() => setFund({ type: "send", to: n().operator!, amount_uhash: (BigInt(fundAmount()) - balance() > 0n ? BigInt(fundAmount()) - balance() : 0n).toString() })} disabled={BigInt(fundAmount()) - balance() <= 0n}>
                            <WalletIcon size={12} /> Fund operator ({formatHash((BigInt(fundAmount()) - balance()).toString(), 0, 2)} HASH from your wallet)
                          </Button>
                        </Show>
                        <p class="text-xs text-muted">The bond is slashed 5 % on proven fraud (failed challenges). Unbonding takes 21 days; the node stops earning while unbonding.</p>
                      </Show>
                    </div>
                  </Card>
                </div>
              </div>

              <Show when={n().running}>
                <Card title="Live status">
                  <div class="grid grid-cols-4 gap-3 p-4">
                    <Stat label="Peers" value={String(num(pick(n().status, "swarm.verified")))} sub={`${num(pick(n().status, "swarm.connected"))} connected · ${num(pick(n().status, "swarm.kad_peers"))} in DHT`} />
                    <Stat label="Uptime" mono={false} value={formatDuration(num(pick(n().status, "uptime_secs")))} sub={`v${str(pick(n().status, "version"))}`} />
                    <Stat label="Storage" value={formatBytes(num(pick(n().status, "blobs.bytes_used")) || num(pick(n().status, "blobs.used_bytes")))} sub={`${num(pick(n().status, "blobs.blobs")) || num(pick(n().status, "blobs.count"))} blobs · ${num(pick(n().status, "mailbox.envelopes"))} envelopes`} />
                    <Stat label="Receipts pending" value={String(num(pick(n().rewards, "pending_receipts")))} sub={`epoch ${str(pick(n().rewards, "epoch"), "—")}`} />
                  </div>
                </Card>
              </Show>

              <div class="grid grid-cols-2 gap-4">
                <Card title="Assignments and challenges (chain)">
                  <div class="p-4 text-sm">
                    <p>Assignments: <span class="mono">{arr(pick(n().assignments, "assignments")).length}</span></p>
                    <p>Challenges: <span class="mono">{arr(pick(n().challenges, "challenges")).length}</span> · passed <span class="mono">{num(pick(n().rewards, "stats.challenges_passed"))}</span> · failed <span class="mono">{num(pick(n().rewards, "stats.challenges_failed"))}</span></p>
                    <p>Fraud score: <span class="mono">{str(pick(n().fraud, "score")) || str(pick(n().fraud, "fraud.score")) || "0"}</span></p>
                  </div>
                </Card>
                <Card title="Earnings">
                  <div class="p-4 text-sm">
                    <p>Credit this epoch: <span class="mono">{str(pick(n().chain_rewards, "credit")) || str(pick(n().chain_rewards, "rewards.credit")) || "0"}</span></p>
                    <p>Lifetime paid: <span class="mono">{formatHash(coin(pick(n().chain_rewards, "paid")) !== "0" ? coin(pick(n().chain_rewards, "paid")) : coin(pick(n().chain_rewards, "rewards.paid")), 2, 6)} HASH</span> → reward address</p>
                    <Show when={current().reward_address}>
                      <p class="mt-1 text-xs text-muted">Payout history: Wallet → History for <Mono text={current().reward_address} head={10} tail={6} /> (received transfers from the reserve).</p>
                    </Show>
                  </div>
                </Card>
              </div>
              <Show when={n().running && provider() === undefined && n().operator && balance() >= minBond}>
                <Badge strong>Bond funded; the node registers itself on its next pass (a minute or two).</Badge>
              </Show>
            </>
          )}
        </Show>
      </Show>

      <Dialog open={cold() !== null} onClose={() => setCold(null)} title="Cold reward address" description="Write these 24 words down. They are NOT stored anywhere by the app; only the address goes into the node configuration." width="max-w-xl" footer={<Button onClick={() => { patch((s) => (s.reward_address = cold()!.address)); setCold(null); }}>Use this address</Button>}>
        <Show when={cold()}>
          <ol class="mono grid grid-cols-3 gap-1.5 select-none" data-secret="true">
            <For each={cold()!.words}>{(w, i) => <li class="flex items-baseline gap-2 rounded-md border border-border bg-surface px-2.5 py-1.5 text-sm"><span class="w-5 text-right text-xs text-muted">{i() + 1}</span><span>{w}</span></li>}</For>
          </ol>
          <p class="mono mt-3 text-xs">{cold()!.address}</p>
        </Show>
      </Dialog>
      <Dialog open={log() !== null} onClose={() => setLog(null)} title="Node log" width="max-w-3xl">
        <pre class="mono selectable max-h-[60vh] overflow-auto whitespace-pre-wrap text-[11px] text-muted">{log() || "(empty — the log appears once the wrapper has started the node)"}</pre>
      </Dialog>
      <TxConfirm spec={fund()} onClose={() => setFund(null)} onSubmitted={() => void refetch()} />
    </div>
  );
}
