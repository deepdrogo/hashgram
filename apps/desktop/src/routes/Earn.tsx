// Earn: the provider lifecycle as plain sentences, earnings, the node
// manager (install / start / stop / log), and the register form. Never
// the word "mining": nodes earn from proven work.
import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Play, Square, Download, Trash2, RefreshCw, Coins, Server, ScrollText } from "lucide-solid";
import { Button, Card, Field, Input, Notice, Stat, Switch, Badge, Dialog, Tabs, Skeleton } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Mono } from "~/components/identity";
import { MnemonicDisplay } from "~/components/MnemonicDisplay";
import { ipc, errText, type NodeSetup, type NodeOverview, type EarnStatus, type Earnings, type ProviderStatus } from "~/lib/ipc";
import { formatHash, isHashAddress, formatBytes } from "~/lib/format";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { confirm } from "~/lib/dialogs";

const ROLES: [string, string][] = [
  ["store", "keep encrypted mailboxes and Drive objects"],
  ["media", "serve blob chunks"],
  ["relay", "relay traffic and chain reads for clients"],
];

export function EarnRoute() {
  const params = useParams<{ tab?: string }>();
  const navigate = useNavigate();
  const tab = () => params.tab || "overview";
  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="border-b border-border px-4">
        <Tabs
          class="border-b-0"
          value={tab()}
          onChange={(v) => navigate(`/earn/${v}`)}
          tabs={[
            { id: "overview", label: t("earn_title") },
            { id: "node", label: t("earn_run_node") },
            { id: "providers", label: "All providers" },
          ]}
        />
      </div>
      <div class="min-h-0 flex-1 overflow-auto">
        <div class="page">
          <Show when={tab() === "overview"}>
            <Overview />
          </Show>
          <Show when={tab() === "node"}>
            <RunNode />
          </Show>
          <Show when={tab() === "providers"}>
            <Providers />
          </Show>
        </div>
      </div>
    </div>
  );
}

function Overview() {
  const [status, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.earnStatus(),
  );
  const [earn] = createResource(
    () => store.ticks().wallet,
    () => ipc.earnEarnings().catch(() => null as Earnings | null),
  );
  const [busy, setBusy] = createSignal(false);
  const s = () => status()?.status;
  const act = async (f: () => Promise<unknown>, done: string) => {
    setBusy(true);
    try {
      await f();
      store.toast(done);
      void refetch();
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={!status.error} fallback={<ErrorState error={status.error} onRetry={() => void refetch()} />}>
      <Show when={status()} fallback={<Skeleton lines={4} />}>
        {(st: () => EarnStatus) => (
          <div class="flex flex-col gap-4">
            <Card title="Provider lifecycle">
              <div class="p-4">
                <div class="flex items-center gap-2">
                  <Coins size={16} class="text-brand" />
                  <span class="text-sm font-semibold">{st().status.lifecycle.replace(/([A-Z])/g, " $1").trim()}</span>
                  <Show when={st().status.moniker}>
                    <Badge>{st().status.moniker}</Badge>
                  </Show>
                </div>
                <p class="mt-2 text-[13px]" data-testid="lifecycle-sentence">
                  {st().sentence}
                </p>
                <Show when={st().status.lifecycle !== "Unregistered"}>
                  <div class="mt-3 grid grid-cols-4 gap-3">
                    <Stat label="Bond" value={`${formatHash(st().status.bond_uhash)} HASH`} />
                    <Stat label="Roles" mono={false} value={st().status.roles.join(", ") || "—"} />
                    <Stat label="Declared storage" value={formatBytes(st().status.declared_storage_bytes)} />
                    <Stat label="Fraud score" value={String(st().status.fraud_score)} sub={st().status.jailed ? `jailed until ${st().status.jailed_until_height}` : undefined} />
                  </div>
                  <p class="mt-2 text-xs text-muted">
                    Reward address: <Mono text={st().status.reward_address} head={12} tail={6} copy />
                  </p>
                  <div class="mt-3 flex gap-2">
                    <Show when={st().status.unbonding_height === 0}>
                      <Button variant="danger" size="sm" loading={busy()} onClick={async () => { if (await confirm("Begin unbonding? The bond is released after 21 days and the node stops earning.")) await act(() => ipc.earnUnbond(), "Unbonding started"); }}>
                        Begin unbonding
                      </Button>
                    </Show>
                    <Show when={st().status.unbonding_height > 0}>
                      <Button variant="secondary" size="sm" loading={busy()} onClick={() => act(() => ipc.earnWithdraw(), "Withdrawal submitted")}>
                        Withdraw bond
                      </Button>
                    </Show>
                  </div>
                </Show>
              </div>
            </Card>
            <Card title="Earnings">
              <div class="grid grid-cols-4 gap-3 p-4">
                <Stat label="Total paid" value={`${formatHash(earn()?.total_paid_uhash ?? "0")} HASH`} />
                <Stat label="Pending credit" value={earn()?.pending_credit ?? "0"} sub="this epoch" />
                <Stat label="Epoch" value={String(earn()?.epoch ?? "—")} />
                <Stat label="Reserve remaining" value={`${formatHash(earn()?.reserve_remaining_uhash ?? "0", 0, 0)} HASH`} sub="network-wide" />
              </div>
              <p class="px-4 pb-4 text-xs text-muted">Rewards come from chain-issued storage challenges answered and client-signed receipts served. Declared capacity earns nothing by itself.</p>
            </Card>
            <Show when={s()?.lifecycle === "Unregistered"}>
              <Notice title="How to earn">Run a node on this PC (next tab), fund its operator address with the bond, and register. The reward address must be a different key from the operator: the operator key is hot.</Notice>
            </Show>
          </div>
        )}
      </Show>
    </Show>
  );
}

function RunNode() {
  const [ov, { refetch }] = createResource(() => ipc.nodeOverview());
  const [busy, setBusy] = createSignal<string | null>(null);
  const [setup, setSetup] = createSignal<NodeSetup | null>(null);
  const [cold, setCold] = createSignal<{ words: string[]; address: string } | null>(null);
  const [log, setLog] = createSignal<string | null>(null);
  const [reg, setReg] = createSignal(false);
  onMount(() => {
    const id = setInterval(() => void refetch(), 10_000);
    onCleanup(() => clearInterval(id));
  });
  const current = (): NodeSetup => setup() ?? ov()?.setup ?? { roles: ["store", "media", "relay"], storage_gib: 20, bandwidth_mbps: 0, reward_address: "", moniker: "home-pc", auto_register: true };
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
      store.toast(errText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  return (
    <Show when={!ov.error} fallback={<ErrorState error={ov.error} onRetry={() => void refetch()} />}>
      <Show when={ov()} fallback={<Skeleton lines={5} />}>
        {(n: () => NodeOverview) => (
          <div class="flex flex-col gap-4">
            <div class="grid grid-cols-4 gap-3">
              <Stat label="Node" mono={false} value={n().running ? "running" : n().configured ? "stopped" : "not set up"} sub={n().registration === "service" ? "Windows service" : n().registration === "scheduled_task" ? "background task (at logon)" : n().bundled ? "not installed" : "node binary not bundled in this build"} />
              <Stat label="Reachability" mono={false} value={n().reachability} sub={n().reachability === "private" ? "behind NAT: fewer assignments" : n().reachability === "public" ? "reachable from the Internet" : "measured once peers connect"} />
              <Stat label="Operator" mono={false} value={n().operator ? <Mono text={n().operator!} head={10} tail={6} copy /> : "—"} sub={n().operator_balance_uhash !== null ? `${formatHash(n().operator_balance_uhash!)} HASH` : "created with the configuration"} />
              <Stat label="Provider on chain" mono={false} value={n().provider?.status.lifecycle.replace(/([A-Z])/g, " $1").trim() ?? "not registered"} sub={n().provider ? `bond ${formatHash(n().provider!.status.bond_uhash, 0, 0)} HASH` : "register once the operator holds the bond"} />
            </div>
            <Show when={n().provider}>
              <Notice>{n().provider!.sentence}</Notice>
            </Show>
            <Notice strong title="What this does and does not promise">
              Your PC stores and serves other people's encrypted data and is paid from the service reserve for work that was proven, up to a per-epoch ceiling. Behind a NAT that hole punching cannot cross it receives fewer assignments and earns less. Earnings depend on demand. Nothing here is guaranteed.
            </Notice>
            <div class="grid grid-cols-2 gap-4">
              <Card title="Setup">
                <div class="flex flex-col gap-3 p-4">
                  <div>
                    <p class="label">Roles</p>
                    <For each={ROLES}>
                      {([role, what]) => <Switch label={role} hint={what} checked={current().roles.includes(role)} onChange={(v) => patch((s) => (s.roles = v ? [...new Set([...s.roles, role])] : s.roles.filter((r) => r !== role)))} />}
                    </For>
                  </div>
                  <div class="grid grid-cols-2 gap-3">
                    <Field label="Disk quota (GiB)">
                      <Input mono type="number" min="1" value={current().storage_gib} onInput={(e) => patch((s) => (s.storage_gib = Number(e.currentTarget.value) || 1))} />
                    </Field>
                    <Field label="Bandwidth cap (Mbit/s, 0 = none)">
                      <Input mono type="number" min="0" value={current().bandwidth_mbps} onInput={(e) => patch((s) => (s.bandwidth_mbps = Number(e.currentTarget.value) || 0))} />
                    </Field>
                  </div>
                  <Field label="Moniker">
                    <Input value={current().moniker} onInput={(e) => patch((s) => (s.moniker = e.currentTarget.value))} placeholder="home-pc" />
                  </Field>
                  <Field label="Reward address" hint="Where earnings are paid. Must differ from the operator: the operator key is hot (it signs receipts on the node); rewards go to a key that only receives." error={current().reward_address && !isHashAddress(current().reward_address) ? "not a hash1… address" : current().reward_address && current().reward_address === n().operator ? "must differ from the operator address" : undefined}>
                    <div class="flex gap-2">
                      <Input mono value={current().reward_address} onInput={(e) => patch((s) => (s.reward_address = e.currentTarget.value.trim()))} placeholder="hash1…" />
                      <Button variant="secondary" onClick={() => void ipc.nodeGenerateColdAddress().then(setCold).catch((e) => store.toast(errText(e), "error"))}>
                        Generate cold
                      </Button>
                    </div>
                  </Field>
                  <Switch label="Register as a provider automatically" hint="Once the operator address holds the bond plus fees, the node submits the registration itself." checked={current().auto_register} onChange={(v) => patch((s) => (s.auto_register = v))} />
                  <div class="flex justify-end">
                    <Button onClick={() => act("save", () => ipc.nodeConfigure(current()), "Node configuration written")} loading={busy() === "save"} disabled={!current().roles.length || (!!current().reward_address && !isHashAddress(current().reward_address))}>
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
                    <div class="flex flex-wrap gap-2">
                      <Button onClick={() => act("install", () => ipc.nodeInstall(), "Installed")} loading={busy() === "install"} disabled={!n().configured || !n().bundled}>
                        <Download size={14} /> Install {n().elevated ? "as a service" : "as a background task"}
                      </Button>
                      <Button variant="secondary" onClick={() => act("start", () => ipc.nodeStart(), "Started")} loading={busy() === "start"} disabled={n().registration === "none"}>
                        <Play size={14} /> Start
                      </Button>
                      <Button variant="secondary" onClick={() => act("stop", () => ipc.nodeStop(), "Stopped")} loading={busy() === "stop"} disabled={n().registration === "none"}>
                        <Square size={14} /> Stop
                      </Button>
                      <Button variant="ghost" onClick={async () => { if (await confirm("Remove the node registration? Data and keys are kept.")) await act("uninstall", () => ipc.nodeUninstall(), "Removed"); }} loading={busy() === "uninstall"} disabled={n().registration === "none"}>
                        <Trash2 size={14} /> Uninstall
                      </Button>
                      <Button variant="ghost" onClick={() => void refetch()}>
                        <RefreshCw size={14} />
                      </Button>
                    </div>
                    <Show when={!n().bundled}>
                      <Notice strong>This build does not bundle hashgram-node.exe; release builds do.</Notice>
                    </Show>
                    <Show when={n().node_id}>
                      <p class="text-xs text-muted">
                        Node key: <Mono text={n().node_id!} head={12} tail={8} copy />
                      </p>
                    </Show>
                    <div class="flex gap-2">
                      <Button variant="secondary" size="sm" onClick={() => void ipc.nodeLogTail(300).then(setLog).catch((e) => store.toast(errText(e), "error"))}>
                        <ScrollText size={12} /> Log
                      </Button>
                      <Button variant="secondary" size="sm" onClick={() => setReg(true)} disabled={!n().operator || !n().node_id}>
                        <Server size={12} /> Register on chain…
                      </Button>
                    </div>
                  </div>
                </Card>
                <Show when={n().status}>
                  <Card title="Live status (node API)">
                    <pre class="mono max-h-56 overflow-auto p-3 text-[11px] text-muted selectable">{JSON.stringify(n().status, null, 1)}</pre>
                  </Card>
                </Show>
              </div>
            </div>
            <Dialog open={!!cold()} onClose={() => setCold(null)} title="Cold reward address" description="Shown once. Write the 24 words down; the app keeps only the address." width="max-w-xl">
              <Show when={cold()}>
                {(c) => (
                  <div class="flex flex-col gap-3">
                    <MnemonicDisplay words={c().words} />
                    <Mono text={c().address} full class="text-xs" />
                    <div class="flex justify-end">
                      <Button onClick={() => { patch((s) => (s.reward_address = c().address)); setCold(null); }}>Use this address</Button>
                    </div>
                  </div>
                )}
              </Show>
            </Dialog>
            <Dialog open={log() !== null} onClose={() => setLog(null)} title="Node log" width="max-w-3xl">
              <pre class="mono max-h-[60vh] overflow-auto text-[11px] selectable">{log() || "(empty)"}</pre>
            </Dialog>
            <RegisterDialog open={reg()} onClose={() => setReg(false)} overview={n()} setup={current()} />
          </div>
        )}
      </Show>
    </Show>
  );
}

function RegisterDialog(props: { open: boolean; onClose: () => void; overview: NodeOverview; setup: NodeSetup }) {
  const [bond, setBond] = createSignal("1000");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  return (
    <Dialog open={props.open} onClose={props.onClose} title="Register as a provider" description="One transaction from your account with the bond. The reward address must differ from the operator." width="max-w-md">
      <div class="flex flex-col gap-3 text-[13px]">
        <p>
          Operator (the node): <Mono text={props.overview.operator ?? ""} head={12} tail={6} />
        </p>
        <p>
          Reward address: <Mono text={props.setup.reward_address || "(not set)"} head={12} tail={6} />
        </p>
        <p>Roles: {props.setup.roles.join(", ")} · Storage: {props.setup.storage_gib} GiB · Moniker: {props.setup.moniker}</p>
        <Field label="Bond (HASH)" hint="Slashed on proven fraud; unbonds over 21 days.">
          <Input mono value={bond()} onInput={(e) => setBond(e.currentTarget.value)} />
        </Field>
        <Show when={error()}>
          <Notice strong>{error()}</Notice>
        </Show>
        <div class="flex justify-end">
          <Button
            loading={busy()}
            disabled={!props.setup.reward_address || !props.overview.node_id}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const r = await ipc.earnRegister(props.setup.reward_address, props.overview.node_id ?? "", props.setup.roles, bond(), props.setup.storage_gib, props.setup.moniker);
                store.toast(`${r.summary} — submitted`);
                props.onClose();
              } catch (e) {
                setError(errText(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            Register
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function Providers() {
  const [list, { refetch }] = createResource(() => ipc.earnProviders());
  return (
    <Show when={!list.error} fallback={<ErrorState error={list.error} onRetry={() => void refetch()} />}>
      <Card title={`Providers on the network (${list()?.length ?? "…"})`}>
        <table class="table">
          <thead>
            <tr>
              <th>Moniker</th>
              <th>Operator</th>
              <th>Roles</th>
              <th class="text-right">Bond</th>
              <th>State</th>
            </tr>
          </thead>
          <tbody>
            <For each={list() ?? []} fallback={<tr><td colSpan={5} class="text-center text-xs text-muted">{list.loading ? t("loading") : "No providers registered."}</td></tr>}>
              {(p: ProviderStatus) => (
                <tr>
                  <td>{p.moniker || "—"}</td>
                  <td>
                    <Mono text={p.operator} head={10} tail={6} />
                  </td>
                  <td class="text-xs text-muted">{p.roles.join(", ")}</td>
                  <td class="tnum text-right">{formatHash(p.bond_uhash, 0, 0)}</td>
                  <td>
                    <Badge brand={p.lifecycle === "Active"} strong={p.lifecycle === "Jailed"}>
                      {p.lifecycle}
                    </Badge>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Card>
    </Show>
  );
}
