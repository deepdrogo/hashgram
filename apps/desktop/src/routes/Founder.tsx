// Founder transparency, live for every user: configured bps and ceiling,
// accrued / paid / pending revenue, realised share from feerouter totals,
// beneficiary history, the beneficiary's vesting and delegations.
import { createMemo, For, Show } from "solid-js";
import { Card, Notice, Skeleton, Stat } from "~/components/ui";
import { Mono, VerifiedBy } from "~/components/identity";
import { useChain, readOf, errorOf } from "~/lib/chain";
import { pick, str, arr, coin } from "~/lib/ipc";
import { formatHash, toBig, formatTime } from "~/lib/format";

export function Founder() {
  const [params] = useChain(() => "hashgram/founder/v1/params");
  const [revenue] = useChain(() => "hashgram/founder/v1/revenue");
  const [history] = useChain(() => "hashgram/founder/v1/beneficiary_history");
  const [totals] = useChain(() => "hashgram/feerouter/v1/totals");
  const beneficiary = () => {
    const p = params();
    return p?.ok ? str(pick(p.value, "params.beneficiary")) || str(pick(p.value, "beneficiary")) : "";
  };
  const [account] = useChain(() => (beneficiary() ? `cosmos/auth/v1beta1/accounts/${beneficiary()}` : null));
  const [delegations] = useChain(() => (beneficiary() ? `cosmos/staking/v1beta1/delegations/${beneficiary()}` : null));

  const shareBps = () => {
    const p = params();
    return p?.ok ? str(pick(p.value, "params.fee_basis_points") ?? pick(p.value, "params.share_bps"), "100") : "—";
  };
  const ceilingBps = () => {
    const p = params();
    return p?.ok ? str(pick(p.value, "max_fee_basis_points") ?? pick(p.value, "params.ceiling_bps"), "100") : "—";
  };
  const payout = () => {
    const p = params();
    return p?.ok
      ? { period: str(pick(p.value, "params.payout_period_blocks"), "?"), min: coin(pick(p.value, "params.min_payout")) }
      : null;
  };
  const rev = createMemo(() => {
    const r = revenue();
    const v = r?.ok ? r.value : null;
    return {
      accrued: coin(pick(v, "total_accrued") ?? pick(v, "accrued")),
      paid: coin(pick(v, "total_paid") ?? pick(v, "paid")),
      pending: coin(pick(v, "pending")),
      lastPayoutHeight: str(pick(v, "last_payout_height"), "0"),
    };
  });
  const realised = createMemo(() => {
    const t = totals();
    if (!t?.ok) return null;
    const v = pick(t.value, "totals") ?? t.value;
    const founder = toBig(coin(pick(v, "founder_share")));
    const total = toBig(coin(pick(v, "total_qualifying")));
    if (total === 0n) return { bps: "0.00", founder, total };
    // 10000 × founder_share / total_qualifying, two decimals.
    return { bps: (Number((founder * 1_000_000n) / total) / 100).toFixed(2), founder, total };
  });
  const vesting = () => {
    const a = account();
    if (!a?.ok) return null;
    const acct = pick(a.value, "account");
    const bva = pick(acct, "base_vesting_account");
    const ov = arr(pick(bva, "original_vesting")).find((c) => str(pick(c, "denom")) === "uhash");
    return {
      type: str(pick(acct, "@type")).split(".").pop() ?? "",
      original: ov ? str(pick(ov, "amount"), "0") : null,
      end: Number(str(pick(bva, "end_time"), "0")),
      start: Number(str(pick(acct, "start_time"), "0")),
    };
  };
  const delegated = () => {
    const d = delegations();
    if (!d?.ok) return null;
    return arr(pick(d.value, "delegation_responses")).map((x) => ({
      validator: str(pick(x, "delegation.validator_address")),
      amount: str(pick(x, "balance.amount"), "0"),
    }));
  };

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-baseline justify-between">
        <h1 class="page-title">Founder</h1>
        <VerifiedBy verification={readOf(params())?.verification} source={readOf(params())?.source} />
      </div>
      <Notice>
        The Founder receives a share of <strong>protocol fee revenue only</strong> — never of transferred principal. 100 HASH sent is 100 HASH received. The share is a hard-coded ceiling of 100 basis points (1 %).
      </Notice>
      <Show when={!params.loading} fallback={<Skeleton lines={3} />}>
        <Show when={params()?.ok} fallback={<Notice strong title="Founder data unavailable">{errorOf(params()) ?? "No chain source answered."}</Notice>}>
          <div class="grid grid-cols-4 gap-3">
            <Stat label="Configured share" value={`${shareBps()} bps`} sub={`ceiling ${ceilingBps()} bps`} />
            <Stat label="Realised share" value={realised() ? `${realised()!.bps} bps` : "—"} sub={realised() ? `${formatHash(realised()!.founder, 2, 6)} / ${formatHash(realised()!.total, 2, 6)} HASH of qualifying fees` : "from feerouter totals"} />
            <Stat label="Accrued (lifetime)" value={`${formatHash(rev().accrued, 2, 6)} HASH`} sub={payout() ? `paid every ${Number(payout()!.period).toLocaleString()} blocks, min ${formatHash(payout()!.min)} HASH` : undefined} />
            <Stat label="Paid / pending" value={`${formatHash(rev().paid, 2, 6)} HASH`} sub={`pending ${formatHash(rev().pending, 2, 6)} HASH${rev().lastPayoutHeight !== "0" ? ` · last payout at ${rev().lastPayoutHeight}` : ""}`} />
          </div>
          <div class="grid grid-cols-2 gap-3">
            <Card title="Beneficiary">
              <div class="flex flex-col gap-2 p-4 text-sm">
                <Mono text={beneficiary()} full copy />
                <Show when={vesting()}>
                  {(v) => (
                    <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
                      <dt class="text-muted">Account</dt>
                      <dd>{v().type}</dd>
                      <Show when={v().original}>
                        <dt class="text-muted">Original vesting</dt>
                        <dd class="mono">{formatHash(v().original!)} HASH</dd>
                        <dt class="text-muted">Vesting</dt>
                        <dd>
                          {formatTime(v().start)} → {formatTime(v().end)}
                        </dd>
                      </Show>
                    </dl>
                  )}
                </Show>
                <p class="text-xs text-muted">Genesis allocation: 199,000,000 HASH — 19,000,000 spendable, 180,000,000 vesting over 96 months.</p>
              </div>
            </Card>
            <Card title="Delegations of the beneficiary">
              <Show when={delegated()} fallback={<div class="p-4"><Skeleton lines={2} /></div>}>
                <Show when={delegated()!.length} fallback={<p class="p-4 text-xs text-muted">None.</p>}>
                  <ul>
                    <For each={delegated()!}>
                      {(d) => (
                        <li class="flex items-center justify-between border-b border-border px-4 py-2 text-sm last:border-0">
                          <Mono text={d.validator} head={16} tail={6} copy />
                          <span class="mono">{formatHash(d.amount)} HASH</span>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </Show>
            </Card>
          </div>
          <Card title="Beneficiary history">
            <Show when={history()?.ok} fallback={<p class="p-4 text-xs text-muted">{errorOf(history()) ?? "Loading…"}</p>}>
              <Show when={arr(pick(history()!.ok ? (history() as { value: unknown }).value : null, "history")).length} fallback={<p class="p-4 text-xs text-muted">The beneficiary has never changed.</p>}>
                <ul>
                  <For each={arr(pick((history() as { value: unknown }).value, "history"))}>
                    {(h) => (
                      <li class="flex items-center justify-between border-b border-border px-4 py-2 text-sm last:border-0">
                        <Mono text={str(pick(h, "beneficiary"))} head={16} tail={6} />
                        <span class="mono text-xs text-muted">height {str(pick(h, "height"))}</span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Show>
          </Card>
        </Show>
      </Show>
    </div>
  );
}
