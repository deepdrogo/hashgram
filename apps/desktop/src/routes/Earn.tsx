// Earn — the network view everyone sees. There is no mining: nodes earn
// for storing and serving real bytes; the budget is a ceiling, not a
// guarantee. "Run a node on this PC" is Stage 3.
import { For, Show, createSignal, type Resource } from "solid-js";
import { Card, Notice, Skeleton, Stat, Button } from "~/components/ui";
import { RunNode } from "./RunNode";
import { VerifiedBy, Mono } from "~/components/identity";
import { useChain, readOf, errorOf, valueOf } from "~/lib/chain";
import { pick, str, arr, coin, num } from "~/lib/ipc";
import { formatHash, toBig, blocksToDuration } from "~/lib/format";

const WHAT_EARNS = [
  ["store", "keeping mailboxes and blob replicas, answering fetches"],
  ["media", "serving blob chunks"],
  ["relay", "relaying traffic between peers"],
  ["bootstrap", "being a first-contact node"],
  ["call", "TURN relaying for calls"],
];

export function Earn() {
  const [params] = useChain(() => "hashgram/serviceproof/v1/params");
  const [reserve] = useChain(() => "hashgram/serviceproof/v1/reserve");
  const [epoch] = useChain(() => "hashgram/serviceproof/v1/epoch/current");
  const [providers] = useChain(() => "hashgram/serviceproof/v1/providers");
  const [schedule] = useChain(() => "hashgram/serviceproof/v1/emission_schedule");
  const [welcome] = useChain(() => "hashgram/welcome/v1/status");

  const remaining = () => toBig(coin(pick(valueOf(reserve()), "remaining")));
  const bonded = () => toBig(coin(pick(valueOf(reserve()), "bonded")));
  const emitted = () => toBig(coin(pick(valueOf(reserve()), "reserve.total_emitted")));
  const projected = () => {
    const p = coin(pick(valueOf(epoch()), "projected_budget"));
    if (p !== "0") return toBig(p);
    // min(remaining × 5/10,000, 250,000 HASH)
    const a = (remaining() * 5n) / 10_000n;
    const cap = 250_000n * 1_000_000n;
    return a < cap ? a : cap;
  };
  const epochNumber = () => str(pick(valueOf(epoch()), "epoch.number"), "—");
  const blocksRemaining = () => num(pick(valueOf(epoch()), "blocks_remaining"));
  const minBond = () => coin(pick(valueOf(params()), "params.min_bond")) !== "0" ? coin(pick(valueOf(params()), "params.min_bond")) : "1000000000";
  const providerList = () => arr(pick(valueOf(providers()), "providers"));
  const scheduleRows = () => arr(pick(valueOf(schedule()), "projections"));
  const w = () => valueOf(welcome());
  const [showNode, setShowNode] = createSignal(false);

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-baseline justify-between">
        <h1 class="page-title">Earn</h1>
        <VerifiedBy verification={readOf(reserve())?.verification} source={readOf(reserve())?.source} />
      </div>
      <Notice strong>There is no mining. Nodes earn for storing and serving real bytes; the budget is a ceiling, not a guarantee.</Notice>
      <Show when={!reserve.loading} fallback={<Skeleton lines={3} />}>
        <div class="grid grid-cols-4 gap-3">
          <Stat label="Service reserve remaining" value={`${formatHash(remaining(), 0, 0)} HASH`} sub={errorOf(reserve()) ?? `of 500,000,000 at genesis · ${formatHash(emitted(), 0, 0)} emitted`} />
          <Stat label="This epoch's budget" value={`${formatHash(projected(), 0, 0)} HASH`} sub="min(remaining × 5 / 10,000, 250,000 HASH)" />
          <Stat label="Per-provider cap" value={`${formatHash(projected() / 20n, 0, 0)} HASH`} sub="5 % of the budget" />
          <Stat label={`Epoch ${epochNumber()}`} value={blocksRemaining() ? `${blocksRemaining().toLocaleString()} blocks left` : "—"} sub={`≈ ${blocksToDuration(blocksRemaining())} · 21,600 blocks per epoch · ${providerList().length} provider(s) · ${formatHash(bonded(), 0, 0)} HASH bonded`} />
        </div>
      </Show>
      <div class="grid grid-cols-2 gap-3">
        <Card title="What earns credit">
          <table class="table">
            <tbody>
              <For each={WHAT_EARNS}>
                {([role, what]) => (
                  <tr>
                    <td class="mono w-28">{role}</td>
                    <td class="text-muted">{what}</td>
                  </tr>
                )}
              </For>
              <tr>
                <td class="mono w-28 text-muted">chain relay</td>
                <td class="text-muted">nothing — a public good like peer exchange</td>
              </tr>
            </tbody>
          </table>
          <p class="px-3 py-2 text-xs text-muted">Bond {formatHash(minBond(), 0, 0)} HASH · slashed 5 % on proven fraud · 21-day unbonding.</p>
        </Card>
        <Card title="Emission schedule">
          <Show when={scheduleRows().length} fallback={<p class="p-4 text-xs text-muted">{errorOf(schedule()) ?? "The chain publishes the per-epoch budget as the reserve declines: 5 basis points of what remains, capped at 250,000 HASH."}</p>}>
            <div class="flex h-24 items-end gap-px px-3 pt-3" role="img" aria-label="emission chart: budget per epoch">
              <For each={scheduleRows().slice(0, 120)}>
                {(r) => {
                  const b = toBig(coin(pick(r, "budget")));
                  const h = Number((b * 100n) / (250_000n * 1_000_000n));
                  return <div class="flex-1 bg-muted" style={{ height: `${Math.max(2, Math.min(100, h))}%` }} title={`epoch ${str(pick(r, "epoch"))}: ${formatHash(b, 0, 0)} HASH, reserve after ${formatHash(coin(pick(r, "reserve_after")), 0, 0)} HASH`} />;
                }}
              </For>
            </div>
            <p class="px-3 py-2 text-xs text-muted">
              Budget per epoch for the next {Math.min(120, scheduleRows().length)} epochs: {formatHash(coin(pick(scheduleRows()[0], "budget")), 0, 0)} → {formatHash(coin(pick(scheduleRows()[Math.min(119, scheduleRows().length - 1)], "budget")), 0, 0)} HASH.
            </p>
          </Show>
        </Card>
      </div>
      <Card title="Providers">
        <Show when={providerList().length} fallback={<p class="p-4 text-xs text-muted">{errorOf(providers()) ?? "No registered providers yet."}</p>}>
          <table class="table">
            <thead>
              <tr>
                <th>Operator</th>
                <th>Moniker</th>
                <th>Roles</th>
                <th>Bond</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              <For each={providerList()}>
                {(p) => (
                  <tr>
                    <td>
                      <Mono text={str(pick(p, "operator"))} head={10} tail={6} copy />
                    </td>
                    <td>{str(pick(p, "moniker"))}</td>
                    <td class="mono text-xs">{arr(pick(p, "roles")).map((r) => str(r).replace("SERVICE_ROLE_", "").toLowerCase()).join(", ")}</td>
                    <td class="mono">{formatHash(coin(pick(p, "bond")), 0, 0)} HASH</td>
                    <td class="text-xs">{str(pick(p, "status")).replace("PROVIDER_STATUS_", "").toLowerCase()}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </Card>
      <div class="grid grid-cols-2 gap-3">
        <Card title="Run a node on this PC">
          <div class="p-4 text-sm text-muted">
            Install hashgram-node as a background service managed from here: roles, disk quota, bandwidth cap, a separate cold reward address, the {formatHash(minBond(), 0, 0)} HASH bond, then live assignments, challenges, fraud score and payouts.
            <div class="mt-3">
              <Button onClick={() => setShowNode((v) => !v)}>{showNode() ? "Hide node setup" : "Set up a node"}</Button>
            </div>
          </div>
        </Card>
        <Card title="Welcome reward">
          <div class="flex flex-col gap-2 p-4 text-sm">
            <Show when={w()} fallback={<p class="text-xs text-muted">{errorOf(welcome()) ?? "Loading…"}</p>}>
              <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
                <dt class="text-muted">Module</dt>
                <dd>{pick(w(), "enabled") === true ? "enabled on chain" : "disabled"}{pick(w(), "exhausted") === true ? " · pool exhausted" : ""}</dd>
                <dt class="text-muted">Next reward</dt>
                <dd class="mono">{formatHash(coin(pick(w(), "next_amount")))} HASH (claim #{str(pick(w(), "next_sequence"))})</dd>
                <dt class="text-muted">Claims paid</dt>
                <dd class="mono">{str(pick(w(), "claims_paid"), "0")} · {formatHash(coin(pick(w(), "total_paid")), 0, 0)} HASH</dd>
                <dt class="text-muted">Pool remaining</dt>
                <dd class="mono">{formatHash(coin(pick(w(), "pool_remaining")), 0, 0)} HASH</dd>
              </dl>
            </Show>
            <Notice>A claim needs an eligibility attestation signed by an attestor on the network. No attestor is reachable from this app yet, so there is no claim button here — and there never will be a fake one.</Notice>
          </div>
        </Card>
      </div>
      <Show when={showNode()}>
        <RunNode />
      </Show>
    </div>
  );
}
