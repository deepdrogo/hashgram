import { For, Show } from "solid-js";
import { Card, Notice, Skeleton, Stat } from "~/components/ui";
import { VerifiedBy } from "~/components/identity";
import { useChain, readOf, errorOf, valueOf } from "~/lib/chain";
import { pick, str, arr, coin } from "~/lib/ipc";
import { formatHash } from "~/lib/format";

const GENESIS = [
  ["Founder", "20 %", "200,000,000"],
  ["Service reserve", "50 %", "500,000,000"],
  ["Treasury", "15 %", "150,000,000"],
  ["Growth", "5 %", "50,000,000"],
  ["Grants", "5 %", "50,000,000"],
  ["Liquidity", "5 %", "50,000,000"],
];

export function Supply() {
  const [supply] = useChain(() => "cosmos/bank/v1beta1/supply/by_denom?denom=uhash");
  const [reserves] = useChain(() => "hashgram/treasury/v1/reserves");
  const [serviceReserve] = useChain(() => "hashgram/serviceproof/v1/reserve");
  const total = () => (supply()?.ok ? str(pick(supply()!.ok ? (supply() as { value: unknown }).value : null, "amount.amount"), "0") : null);
  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-baseline justify-between">
        <h1 class="page-title">Supply</h1>
        <VerifiedBy verification={readOf(supply())?.verification} source={readOf(supply())?.source} />
      </div>
      <div class="grid grid-cols-3 gap-3">
        <Show when={!supply.loading} fallback={<div class="stat"><Skeleton lines={2} /></div>}>
          <Stat label="Total supply" value={total() !== null ? `${formatHash(total()!, 0, 6)} HASH` : "—"} sub={errorOf(supply()) ?? "ceiling 1,000,000,000 HASH · no mint module"} />
        </Show>
        <Stat label="Service reserve" value={serviceReserve()?.ok ? `${formatHash(coin(pick(valueOf(serviceReserve()), "remaining")), 0, 0)} HASH` : "—"} sub="pays nodes for storing and serving bytes" />
        <Stat label="Treasury reserves" mono={false} value={reserves()?.ok ? `${arr(pick((reserves() as { value: unknown }).value, "reserves")).length} reserve(s)` : "—"} />
      </div>
      <Notice>Fixed supply. There is no mint module and no mining: the only way HASH moves is transfers, fees, staking rewards drawn from fees, and service rewards paid out of the reserve created at genesis.</Notice>
      <Card title="Genesis allocation">
        <table class="table">
          <thead>
            <tr>
              <th>Allocation</th>
              <th>Share</th>
              <th>HASH</th>
            </tr>
          </thead>
          <tbody>
            <For each={GENESIS}>
              {([name, share, amount]) => (
                <tr>
                  <td>{name}</td>
                  <td class="mono">{share}</td>
                  <td class="mono">{amount}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Card>
      <Show when={reserves()?.ok && arr(pick((reserves() as { value: unknown }).value, "reserves")).length}>
        <Card title="Treasury reserves">
          <ul>
            <For each={arr(pick((reserves() as { value: unknown }).value, "reserves"))}>
              {(r) => (
                <li class="flex items-center justify-between border-b border-border px-4 py-2 text-sm last:border-0">
                  <span>{str(pick(r, "reserve.name")) || str(pick(r, "name"))} <span class="text-xs text-muted">{str(pick(r, "reserve.description"))}</span></span>
                  <span class="mono">{formatHash(coin(pick(r, "balance")), 0, 0)} HASH</span>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>
    </div>
  );
}
