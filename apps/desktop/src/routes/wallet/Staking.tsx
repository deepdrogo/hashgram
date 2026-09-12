// Staking: validators, delegate / undelegate / redelegate, rewards and
// withdraw. The 21-day unbonding and slashing figures are shown before
// confirm (the confirm dialog carries them as warnings from Rust).
import { createMemo, createSignal, For, Show } from "solid-js";
import { Card, Button, Field, Input, Notice, Skeleton, Empty, Badge } from "~/components/ui";
import { Mono, VerifiedBy } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { useChain, readOf, valueOf, errorOf } from "~/lib/chain";
import { pick, str, arr, type MsgSpec } from "~/lib/ipc";
import { formatHash, parseHashInput, toBig } from "~/lib/format";

interface Validator {
  operator: string;
  moniker: string;
  status: string;
  jailed: boolean;
  tokens: string;
  commission: string;
}

export function Staking(props: { address: string | undefined; onChanged: () => void }) {
  const [validators, { refetch: refetchV }] = useChain(() => "cosmos/staking/v1beta1/validators?status=BOND_STATUS_BONDED&pagination.limit=200");
  const [delegations, { refetch: refetchD }] = useChain(() => (props.address ? `cosmos/staking/v1beta1/delegations/${props.address}` : null));
  const [rewards, { refetch: refetchR }] = useChain(() => (props.address ? `cosmos/distribution/v1beta1/delegators/${props.address}/rewards` : null));
  const [unbonding, { refetch: refetchU }] = useChain(() => (props.address ? `cosmos/staking/v1beta1/delegators/${props.address}/unbonding_delegations` : null));
  const [params] = useChain(() => "cosmos/staking/v1beta1/params");
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const [action, setAction] = createSignal<{ kind: "delegate" | "undelegate" | "redelegate"; validator: string } | null>(null);
  const [amount, setAmount] = createSignal("");
  const [target, setTarget] = createSignal("");

  const vals = createMemo<Validator[]>(() => {
    const v = validators();
    if (!v?.ok) return [];
    return arr(pick(v.value, "validators"))
      .map((x) => ({
        operator: str(pick(x, "operator_address")),
        moniker: str(pick(x, "description.moniker"), "(no name)"),
        status: str(pick(x, "status")),
        jailed: pick(x, "jailed") === true,
        tokens: str(pick(x, "tokens"), "0"),
        commission: str(pick(x, "commission.commission_rates.rate"), "0"),
      }))
      .sort((a, b) => (toBig(b.tokens) > toBig(a.tokens) ? 1 : -1));
  });
  const myDelegations = createMemo(() => {
    const d = delegations();
    if (!d?.ok) return new Map<string, string>();
    const m = new Map<string, string>();
    for (const x of arr(pick(d.value, "delegation_responses"))) {
      m.set(str(pick(x, "delegation.validator_address")), str(pick(x, "balance.amount"), "0"));
    }
    return m;
  });
  const myRewards = createMemo(() => {
    const r = rewards();
    if (!r?.ok) return new Map<string, string>();
    const m = new Map<string, string>();
    for (const x of arr(pick(r.value, "rewards"))) {
      const coin = arr(pick(x, "reward")).find((c) => str(pick(c, "denom")) === "uhash");
      m.set(str(pick(x, "validator_address")), coin ? str(pick(coin, "amount"), "0").split(".")[0] ?? "0" : "0");
    }
    return m;
  });
  const totalRewards = createMemo(() => [...myRewards().values()].reduce((a, b) => a + toBig(b), 0n));
  const totalDelegated = createMemo(() => [...myDelegations().values()].reduce((a, b) => a + toBig(b), 0n));
  const unbondingTime = () => {
    const p = params();
    if (!p?.ok) return "21 days";
    const s = str(pick(p.value, "params.unbonding_time"), "1814400s");
    const secs = Number(s.replace("s", ""));
    return Number.isFinite(secs) ? `${Math.round(secs / 86400)} days` : s;
  };

  const refetchAll = () => {
    void refetchV();
    void refetchD();
    void refetchR();
    void refetchU();
    props.onChanged();
  };

  const submitAction = () => {
    const a = action();
    const u = parseHashInput(amount());
    if (!a || !u || u <= 0n) return;
    if (a.kind === "delegate") setSpec({ type: "delegate", validator: a.validator, amount_uhash: u.toString() });
    if (a.kind === "undelegate") setSpec({ type: "undelegate", validator: a.validator, amount_uhash: u.toString() });
    if (a.kind === "redelegate") setSpec({ type: "redelegate", from_validator: a.validator, to_validator: target(), amount_uhash: u.toString() });
  };

  return (
    <div class="flex flex-col gap-4">
      <div class="grid grid-cols-3 gap-3">
        <div class="stat">
          <span class="stat-label">Delegated</span>
          <span class="stat-value">{formatHash(totalDelegated())} HASH</span>
        </div>
        <div class="stat">
          <span class="stat-label">Rewards to withdraw</span>
          <span class="stat-value">{formatHash(totalRewards())} HASH</span>
          <Show when={totalRewards() > 0n}>
            <div class="flex gap-2 pt-1">
              <For each={[...myRewards().entries()].filter(([, v]) => toBig(v) > 0n)}>
                {([v]) => (
                  <Button size="sm" variant="secondary" onClick={() => setSpec({ type: "withdraw_rewards", validator: v })}>
                    Withdraw from {vals().find((x) => x.operator === v)?.moniker ?? "validator"}
                  </Button>
                )}
              </For>
            </div>
          </Show>
        </div>
        <div class="stat">
          <span class="stat-label">Unbonding period</span>
          <span class="stat-value">{unbondingTime()}</span>
          <span class="text-xs text-muted">slashing: 5 % double-sign · 0.01 % downtime</span>
        </div>
      </div>

      <Notice strong title="Before you delegate">
        Undelegating takes {unbondingTime()}; during that time the tokens earn nothing and cannot be moved. Validators that double-sign are slashed 5 %, validators that go down 0.01 %, and delegators share those losses. The confirm screen repeats this.
      </Notice>

      <Show when={unbonding()?.ok && arr(pick(valueOf(unbonding()), "unbonding_responses")).length}>
        <Card title="Unbonding">
          <ul>
            <For each={arr(pick(valueOf(unbonding()), "unbonding_responses"))}>
              {(u) => (
                <For each={arr(pick(u, "entries"))}>
                  {(e) => (
                    <li class="flex items-center justify-between border-b border-border px-4 py-2 text-sm last:border-0">
                      <Mono text={str(pick(u, "validator_address"))} head={14} tail={6} />
                      <span class="mono">{formatHash(str(pick(e, "balance"), "0"))} HASH</span>
                      <span class="text-xs text-muted">completes {str(pick(e, "completion_time"))}</span>
                    </li>
                  )}
                </For>
              )}
            </For>
          </ul>
        </Card>
      </Show>

      <Card
        title="Validators"
        actions={
          <Show when={validators()?.ok}>
            <VerifiedBy verification={readOf(validators())?.verification} source={readOf(validators())?.source} />
          </Show>
        }
      >
        <Show when={!validators.loading} fallback={<div class="p-4"><Skeleton lines={5} /></div>}>
          <Show when={vals().length} fallback={<Empty title="No validators readable">{errorOf(validators()) ?? "The chain returned no bonded validators."}</Empty>}>
            <table class="table">
              <thead>
                <tr>
                  <th>Validator</th>
                  <th>Voting power</th>
                  <th>Commission</th>
                  <th>My stake</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                <For each={vals()}>
                  {(v) => (
                    <tr class="row-hover">
                      <td>
                        <div class="flex flex-col">
                          <span class="flex items-center gap-2">
                            {v.moniker}
                            <Show when={v.jailed}>
                              <Badge strong>jailed</Badge>
                            </Show>
                          </span>
                          <Mono text={v.operator} head={16} tail={6} copy class="text-xs text-muted" />
                        </div>
                      </td>
                      <td class="mono">{formatHash(v.tokens, 0, 0)} HASH</td>
                      <td class="mono">{(Number(v.commission) * 100).toFixed(1)} %</td>
                      <td class="mono">{myDelegations().has(v.operator) ? `${formatHash(myDelegations().get(v.operator)!)} HASH` : "—"}</td>
                      <td class="text-right">
                        <div class="flex justify-end gap-1">
                          <Button size="sm" variant="secondary" onClick={() => { setAction({ kind: "delegate", validator: v.operator }); setAmount(""); }}>
                            Delegate
                          </Button>
                          <Show when={myDelegations().has(v.operator)}>
                            <Button size="sm" variant="ghost" onClick={() => { setAction({ kind: "undelegate", validator: v.operator }); setAmount(""); }}>
                              Undelegate
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => { setAction({ kind: "redelegate", validator: v.operator }); setAmount(""); setTarget(""); }}>
                              Redelegate
                            </Button>
                          </Show>
                        </div>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </Show>
      </Card>

      <Show when={action()}>
        {(a) => (
          <Card title={`${a().kind[0]!.toUpperCase()}${a().kind.slice(1)} — ${vals().find((v) => v.operator === a().validator)?.moniker ?? ""}`}>
            <div class="flex flex-col gap-3 p-4">
              <Show when={a().kind === "redelegate"}>
                <Field label="To validator">
                  <select class="input" value={target()} onChange={(e) => setTarget(e.currentTarget.value)}>
                    <option value="">Choose…</option>
                    <For each={vals().filter((v) => v.operator !== a().validator)}>{(v) => <option value={v.operator}>{v.moniker}</option>}</For>
                  </select>
                </Field>
              </Show>
              <Field label="Amount (HASH)" hint={a().kind !== "delegate" ? `staked here: ${formatHash(myDelegations().get(a().validator) ?? "0")} HASH` : undefined}>
                <Input mono value={amount()} onInput={(e) => setAmount(e.currentTarget.value)} placeholder="0.000000" inputmode="decimal" />
              </Field>
              <div class="flex justify-end gap-2">
                <Button variant="ghost" onClick={() => setAction(null)}>
                  Cancel
                </Button>
                <Button onClick={submitAction} disabled={!parseHashInput(amount()) || (a().kind === "redelegate" && !target())}>
                  Review
                </Button>
              </div>
            </div>
          </Card>
        )}
      </Show>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onSubmitted={() => { setAction(null); refetchAll(); }} />
      <p class="text-xs text-muted">Rewards accrue per validator on chain; the app never rounds them into your balance until you withdraw. Validators are listed by voting power, not recommended.</p>
    </div>
  );
}
