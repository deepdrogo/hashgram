// Wallet: balance with the verification note, send with fee preview,
// receive (QR), history, staking, usernames (availability reasons),
// identity & devices, vesting, Founder transparency. Amounts are strings
// of uhash end to end; never a float.
import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Send, QrCode, AtSign, Smartphone, Copy as CopyIcon, ShieldCheck, ShieldAlert, RefreshCw, Plus } from "lucide-solid";
import { Button, Card, Dialog, Field, Input, Notice, Stat, Tabs, Badge, Skeleton } from "~/components/ui";
import { OfflineBanner, ErrorState } from "~/components/States";
import { Mono } from "~/components/identity";
import { ipc, errText, type TxPreview, type MsgSpec, type PendingRow, type UsernameAvailability, type IdentityStatus } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatHash, parseAmount, isHashAddress, formatTime, truncateMiddle, bps } from "~/lib/format";
import { copyText } from "~/lib/clipboard";
import { useChain, pick, str, arr, coin, num } from "~/lib/chain";
import { confirm } from "~/lib/dialogs";
import { IdentityStep } from "~/routes/Onboarding";

type Tab = "overview" | "send" | "receive" | "history" | "staking" | "usernames" | "devices" | "founder";

export function WalletRoute() {
  const params = useParams<{ tab?: string }>();
  const navigate = useNavigate();
  const tab = (): Tab => (params.tab as Tab) || "overview";
  return (
    <div class="flex h-full flex-col">
      <OfflineBanner />
      <div class="border-b border-border px-4">
        <Tabs
          class="border-b-0"
          value={tab()}
          onChange={(v) => navigate(`/wallet/${v}`)}
          tabs={[
            { id: "overview", label: t("wallet_balance") },
            { id: "send", label: t("wallet_send") },
            { id: "receive", label: t("wallet_receive") },
            { id: "history", label: t("wallet_history"), badge: store.pendingTx() },
            { id: "staking", label: t("wallet_staking") },
            { id: "usernames", label: t("wallet_usernames") },
            { id: "devices", label: t("wallet_devices") },
            { id: "founder", label: "Founder" },
          ]}
        />
      </div>
      <div class="min-h-0 flex-1 overflow-auto">
        <div class="page">
          <Show when={tab() === "overview"}><Overview /></Show>
          <Show when={tab() === "send"}><SendTab /></Show>
          <Show when={tab() === "receive"}><ReceiveTab /></Show>
          <Show when={tab() === "history"}><HistoryTab /></Show>
          <Show when={tab() === "staking"}><StakingTab /></Show>
          <Show when={tab() === "usernames"}><UsernamesTab /></Show>
          <Show when={tab() === "devices"}><DevicesTab /></Show>
          <Show when={tab() === "founder"}><FounderTab /></Show>
        </div>
      </div>
    </div>
  );
}

function VerificationNote(props: { text: string | null | undefined }) {
  const ok = () => !!props.text && !props.text.includes("only one") && !props.text.includes("unverified") && !props.text.includes("single");
  return (
    <span class={`inline-flex items-center gap-1 text-xs ${ok() ? "text-fg" : "text-muted"}`}>
      <Show when={ok()} fallback={<ShieldAlert size={11} />}>
        <ShieldCheck size={11} class="text-brand" />
      </Show>
      {props.text ?? "not verified yet"}
    </span>
  );
}

function Overview() {
  const [ov, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletOverview(),
  );
  return (
    <Show when={!ov.error} fallback={<ErrorState error={ov.error} onRetry={() => void refetch()} />}>
      <Show when={ov()} fallback={<Skeleton lines={4} />}>
        {(o) => (
          <div class="flex flex-col gap-4">
            <Card>
              <div class="flex items-start justify-between p-4">
                <div>
                  <p class="text-xs text-muted">{t("wallet_balance")}</p>
                  <p class="tnum text-3xl font-semibold">
                    {formatHash(o().balance.uhash)} <span class="text-base font-normal text-muted">HASH</span>
                  </p>
                  <VerificationNote text={o().balance.verification} />
                </div>
                <div class="text-right text-xs text-muted">
                  <Mono text={o().address} head={14} tail={8} copy />
                  <p class="mt-1">{o().username ? `@${o().username} · ${o().username}@hashgram.io` : "no username yet"}</p>
                  <Show when={!o().can_sign}>
                    <p class="mt-1 text-fg">This device holds no wallet key: it can read but not sign.</p>
                  </Show>
                </div>
              </div>
            </Card>
            <Show when={!o().account_exists}>
              <Notice title="New account">This address has never received HASH, so the chain has no account for it yet. The balance is 0; that is not an error. Receive HASH from the Receive tab.</Notice>
            </Show>
            <Show when={o().vesting_type}>
              <Card title="Vesting">
                <div class="grid grid-cols-3 gap-3 p-4">
                  <Stat label="Type" mono={false} value={o().vesting_type!.split(".").pop() ?? ""} />
                  <Stat label="Original vesting" value={`${formatHash(o().original_vesting_uhash ?? "0")} HASH`} />
                  <Stat label="Schedule" mono={false} value={`${formatTime(o().vesting_start)} → ${formatTime(o().vesting_end)}`} />
                </div>
              </Card>
            </Show>
            <div class="grid grid-cols-3 gap-3">
              <Stat label="Account number" value={o().account_number?.toString() ?? "—"} />
              <Stat label="Sequence" value={o().sequence?.toString() ?? "—"} />
              <Stat label="Unit" mono={false} value="1 HASH = 1,000,000 uhash" sub="integers only; nothing is rounded" />
            </div>
          </div>
        )}
      </Show>
    </Show>
  );
}

/** Confirm dialog shared by every transaction: preview → warnings → submit. */
export function TxConfirm(props: { spec: MsgSpec | null; memo?: string; onClose: () => void; onDone?: () => void }) {
  const [preview] = createResource(
    () => props.spec,
    (spec) => ipc.txPreview(spec),
  );
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  return (
    <Dialog open={!!props.spec} onClose={props.onClose} title="Confirm transaction" width="max-w-md">
      <Show when={!preview.error} fallback={<Notice strong>{errText(preview.error)}</Notice>}>
        <Show when={preview()} fallback={<Skeleton lines={3} />}>
          {(p: () => TxPreview) => (
            <div class="flex flex-col gap-3 text-[13px]">
              <p class="font-medium">{p().summary}</p>
              <For each={p().warnings}>{(w) => <Notice>{w}</Notice>}</For>
              <div class="grid grid-cols-2 gap-3">
                <Stat label="Network fee" value={p().fee_display} sub={p().simulated ? "simulated" : "estimated (relay cannot simulate)"} />
                <Stat label="Gas limit" value={p().gas_limit.toLocaleString()} />
              </div>
              <Show when={error()}>
                <Notice strong>{error()}</Notice>
              </Show>
              <div class="flex justify-end gap-2">
                <Button variant="ghost" onClick={props.onClose}>
                  {t("cancel")}
                </Button>
                <Button
                  loading={busy()}
                  onClick={async () => {
                    setBusy(true);
                    setError(null);
                    try {
                      const r = await ipc.txSubmit(props.spec!, props.memo ?? "");
                      store.toast(`${r.summary} — submitted (${truncateMiddle(r.hash, 8, 6)})`);
                      store.bump("wallet");
                      props.onDone?.();
                      props.onClose();
                    } catch (e) {
                      setError(errText(e));
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  Sign and send
                </Button>
              </div>
            </div>
          )}
        </Show>
      </Show>
    </Dialog>
  );
}

function SendTab() {
  const [to, setTo] = createSignal("");
  const [amount, setAmount] = createSignal("");
  const [memo, setMemo] = createSignal("");
  const [resolved, setResolved] = createSignal<string | null>(null);
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const uhash = createMemo(() => parseAmount(amount()));
  const resolve = async () => {
    const v = to().trim();
    if (!v) return setResolved(null);
    if (isHashAddress(v)) return setResolved(v);
    try {
      setResolved((await ipc.peopleResolve(v)).address);
    } catch (e) {
      setResolved(null);
      store.toast(errText(e), "error");
    }
  };
  return (
    <div class="max-w-lg">
      <Card title={t("wallet_send")}>
        <div class="flex flex-col gap-3 p-4">
          <Field label="To" hint={resolved() && !isHashAddress(to()) ? `→ ${resolved()}` : "hash1… address or @username"}>
            <Input mono value={to()} onInput={(e) => setTo(e.currentTarget.value)} onBlur={resolve} placeholder="hash1… or @name" />
          </Field>
          <Field label="Amount (HASH)" hint={uhash() !== null ? `${uhash()!.toLocaleString()} uhash` : "up to 6 decimals"} error={amount() && uhash() === null ? "not a valid amount" : undefined}>
            <Input mono value={amount()} onInput={(e) => setAmount(e.currentTarget.value)} placeholder="0.000000" />
          </Field>
          <Field label="Memo (optional, public on chain)">
            <Input value={memo()} onInput={(e) => setMemo(e.currentTarget.value)} maxLength={256} />
          </Field>
          <Notice>Transfers are untaxed: 100 HASH sent is 100 HASH received. 1 % of the network fee (not of the amount) goes to the Founder.</Notice>
          <div class="flex justify-end">
            <Button disabled={!resolved() || !uhash() || uhash() === 0n} onClick={async () => { await resolve(); if (resolved() && uhash()) setSpec({ type: "send", to: resolved()!, amount_uhash: uhash()!.toString() }); }}>
              <Send size={14} /> Review
            </Button>
          </div>
        </div>
      </Card>
      <TxConfirm spec={spec()} memo={memo()} onClose={() => setSpec(null)} onDone={() => { setAmount(""); setMemo(""); }} />
    </div>
  );
}

function ReceiveTab() {
  const address = () => store.status()?.address ?? "";
  const [qr] = createResource(address, (a) => (a ? ipc.qrSvg(a) : Promise.resolve("")));
  return (
    <div class="flex max-w-lg flex-col gap-4">
      <Card title={t("wallet_receive")}>
        <div class="flex items-center gap-5 p-4">
          <div class="h-40 w-40 shrink-0 rounded bg-fg p-2" innerHTML={qr() ?? ""} aria-label="address as QR" />
          <div class="min-w-0 text-[13px]">
            <p class="text-xs text-muted">Your address</p>
            <Mono text={address()} full copy class="break-all text-xs" />
            <p class="mt-2 text-xs text-muted">Sharing it is safe: it is public by construction. Anyone who has it can send you HASH or mail.</p>
            <div class="mt-2 flex gap-2">
              <Button variant="secondary" size="sm" onClick={() => void copyText(address())}>
                <CopyIcon size={12} /> {t("copy")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => void copyText(`hashgram://user/${address()}`)}>
                <QrCode size={12} /> Copy link
              </Button>
            </div>
          </div>
        </div>
      </Card>
    </div>
  );
}

function HistoryTab() {
  const [recent, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.txRecent(),
  );
  const [chain] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletHistory(50).catch(() => null),
  );
  const incoming = createMemo(() => arr(pick(chain(), "tx_responses")));
  return (
    <div class="flex flex-col gap-4">
      <Card title="Submitted from this PC" actions={<Button variant="ghost" size="icon-sm" onClick={() => void refetch()}><RefreshCw size={13} /></Button>}>
        <table class="table">
          <thead>
            <tr>
              <th>When</th>
              <th>What</th>
              <th>State</th>
              <th>Hash</th>
            </tr>
          </thead>
          <tbody>
            <For each={recent() ?? []} fallback={<tr><td colSpan={4} class="text-center text-xs text-muted">Nothing submitted yet.</td></tr>}>
              {(r: PendingRow) => (
                <tr>
                  <td class="tnum text-xs text-muted">{formatTime(r.submitted)}</td>
                  <td>{r.summary}</td>
                  <td>
                    <Badge brand={r.state === "committed"} strong={r.state === "failed"} title={r.raw_log}>
                      {r.state}
                      {r.height ? ` @ ${r.height}` : ""}
                    </Badge>
                  </td>
                  <td>
                    <Mono text={r.hash} head={8} tail={6} copy />
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Card>
      <Card title="Received (from the chain's transaction index)">
        <Show when={chain()} fallback={<p class="p-4 text-xs text-muted">{chain.loading ? t("loading") : "The relay did not answer a transaction search; this needs a node with the tx index enabled."}</p>}>
          <table class="table">
            <thead>
              <tr>
                <th>Height</th>
                <th>Hash</th>
                <th>Result</th>
              </tr>
            </thead>
            <tbody>
              <For each={incoming()} fallback={<tr><td colSpan={3} class="text-center text-xs text-muted">No incoming transfers indexed.</td></tr>}>
                {(x) => (
                  <tr>
                    <td class="tnum">{str(pick(x, "height"))}</td>
                    <td>
                      <Mono text={str(pick(x, "txhash"))} head={8} tail={6} copy />
                    </td>
                    <td>{num(pick(x, "code")) === 0 ? <Badge brand>ok</Badge> : <Badge strong>code {str(pick(x, "code"))}</Badge>}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </Card>
    </div>
  );
}

function StakingTab() {
  const [validators, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.networkValidators(),
  );
  const [delegations] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletDelegations().catch(() => null),
  );
  const [rewards] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletRewards().catch(() => null),
  );
  const [unbonding] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletUnbonding().catch(() => null),
  );
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const [target, setTarget] = createSignal<{ v: string; mode: "delegate" | "undelegate" } | null>(null);
  const [amount, setAmount] = createSignal("");
  const mine = createMemo(() => arr(pick(delegations(), "delegation_responses")).map((d) => ({ validator: str(pick(d, "delegation.validator_address")), amount: str(pick(d, "balance.amount"), "0") })));
  const rewardOf = (v: string) => coin(pick(arr(pick(rewards(), "rewards")).find((r) => str(pick(r, "validator_address")) === v), "reward"));
  const unb = createMemo(() => arr(pick(unbonding(), "unbonding_responses")));
  return (
    <div class="flex flex-col gap-4">
      <Notice>Undelegating takes 21 days, during which the tokens earn nothing and cannot move. Validators are slashed for double-signing (5 %) and downtime (0.01 %); delegators share the loss.</Notice>
      <Show when={mine().length}>
        <Card title="My delegations">
          <table class="table">
            <thead>
              <tr>
                <th>Validator</th>
                <th class="text-right">Delegated</th>
                <th class="text-right">Rewards</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For each={mine()}>
                {(d) => (
                  <tr>
                    <td>
                      <Mono text={d.validator} head={16} tail={6} copy />
                    </td>
                    <td class="tnum text-right">{formatHash(d.amount)} HASH</td>
                    <td class="tnum text-right">{formatHash(rewardOf(d.validator))} HASH</td>
                    <td class="text-right">
                      <Button variant="ghost" size="sm" onClick={() => setSpec({ type: "withdraw_rewards", validator: d.validator })}>
                        Withdraw rewards
                      </Button>
                      <Button variant="ghost" size="sm" onClick={() => setTarget({ v: d.validator, mode: "undelegate" })}>
                        Undelegate
                      </Button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Card>
      </Show>
      <Show when={unb().length}>
        <Card title="Unbonding">
          <ul>
            <For each={unb()}>
              {(u) => (
                <li class="border-b border-border px-4 py-2 text-xs last:border-0">
                  <Mono text={str(pick(u, "validator_address"))} head={16} tail={6} /> ·{" "}
                  <For each={arr(pick(u, "entries"))}>{(e) => <span class="mr-2 tnum">{formatHash(str(pick(e, "balance"), "0"))} HASH until {str(pick(e, "completion_time")).slice(0, 10)}</span>}</For>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>
      <Card title="Validators" actions={<Button variant="ghost" size="icon-sm" onClick={() => void refetch()}><RefreshCw size={13} /></Button>}>
        <Show when={!validators.error} fallback={<ErrorState error={validators.error} onRetry={() => void refetch()} compact />}>
          <table class="table">
            <thead>
              <tr>
                <th>Moniker</th>
                <th>Operator</th>
                <th class="text-right">Voting power</th>
                <th class="text-right">Commission</th>
                <th>Status</th>
                <th />
              </tr>
            </thead>
            <tbody>
              <For each={validators() ?? []} fallback={<tr><td colSpan={6} class="text-center text-xs text-muted">{validators.loading ? t("loading") : "No validators returned."}</td></tr>}>
                {(v) => (
                  <tr>
                    <td>{str(pick(v, "description.moniker"), "—")}</td>
                    <td>
                      <Mono text={str(pick(v, "operator_address"))} head={14} tail={6} copy />
                    </td>
                    <td class="tnum text-right">{formatHash(str(pick(v, "tokens"), "0"), 0, 0)}</td>
                    <td class="tnum text-right">{bps(Math.round(Number(str(pick(v, "commission.commission_rates.rate"), "0")) * 10000))}</td>
                    <td>
                      <Badge brand={str(pick(v, "status")) === "BOND_STATUS_BONDED"} strong={!!pick(v, "jailed")}>
                        {pick(v, "jailed") ? "jailed" : str(pick(v, "status")).replace("BOND_STATUS_", "").toLowerCase()}
                      </Badge>
                    </td>
                    <td class="text-right">
                      <Button variant="secondary" size="sm" onClick={() => setTarget({ v: str(pick(v, "operator_address")), mode: "delegate" })} disabled={!!pick(v, "jailed")}>
                        Delegate
                      </Button>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </Card>
      <Dialog open={!!target()} onClose={() => setTarget(null)} title={target()?.mode === "delegate" ? "Delegate" : "Undelegate"} width="max-w-sm">
        <div class="flex flex-col gap-3">
          <Mono text={target()?.v ?? ""} head={18} tail={8} />
          <Field label="Amount (HASH)" error={amount() && parseAmount(amount()) === null ? "not a valid amount" : undefined}>
            <Input mono value={amount()} onInput={(e) => setAmount(e.currentTarget.value)} placeholder="0.000000" />
          </Field>
          <div class="flex justify-end">
            <Button
              disabled={!parseAmount(amount())}
              onClick={() => {
                const tg = target()!;
                const u = parseAmount(amount())!.toString();
                setSpec(tg.mode === "delegate" ? { type: "delegate", validator: tg.v, amount_uhash: u } : { type: "undelegate", validator: tg.v, amount_uhash: u });
                setTarget(null);
              }}
            >
              Review
            </Button>
          </div>
        </div>
      </Dialog>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onDone={() => setAmount("")} />
    </div>
  );
}

const REASONS: Record<string, string> = {
  taken: "already registered",
  reserved: "reserved by the network",
  confusable_with: "looks too much like an existing name",
  invalid: "letters, digits, underscore, dot and hyphen only",
  too_short: "too short (3 or more)",
  too_long: "too long (32 at most)",
  mixed_script: "mixes scripts",
  non_ascii_not_allowed: "only ASCII is allowed",
};

function UsernamesTab() {
  const [mine, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.walletUsernames().catch(() => null),
  );
  const [name, setName] = createSignal("");
  const [avail] = createResource(
    () => name().trim().toLowerCase(),
    (n) => (n.length >= 1 ? ipc.walletUsernameAvailability(n).catch(() => null) : Promise.resolve(null as UsernameAvailability | null)),
  );
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const registered = () => store.identity()?.this_device_registered !== false;
  const regs = createMemo(() => arr(pick(mine(), "registrations")));
  return (
    <div class="flex max-w-2xl flex-col gap-4">
      <Card title="My usernames" actions={<Button variant="ghost" size="icon-sm" onClick={() => void refetch()}><RefreshCw size={13} /></Button>}>
        <ul>
          <For each={regs()} fallback={<li class="p-4 text-xs text-muted">No username yet. People can always reach you by address; a name makes it readable.</li>}>
            {(r) => (
              <li class="flex items-center gap-3 border-b border-border px-4 py-2 text-[13px] last:border-0">
                <AtSign size={13} class="text-brand" />
                <span class="mono flex-1">@{str(pick(r, "name"))}</span>
                <span class="tnum text-xs text-muted">expires at height {str(pick(r, "expiry_height"))}</span>
                <Button variant="secondary" size="sm" onClick={() => setSpec({ type: "renew_username", name: str(pick(r, "name")) })}>
                  Renew
                </Button>
              </li>
            )}
          </For>
        </ul>
      </Card>
      <Card title="Register a username">
        <div class="flex flex-col gap-3 p-4">
          <Show when={!registered()}>
            <Notice strong title="Register this PC first">
              Usernames are on-chain records. Receive a small amount of HASH and complete the one-time identity registration in Wallet → Devices first.
              <Button class="mt-2" size="sm" variant="secondary" onClick={() => { window.location.hash = "/wallet/devices"; }}>
                Open Devices
              </Button>
            </Notice>
          </Show>
          <Field
            label="Name"
            hint={avail() ? (avail()!.available ? `@${avail()!.normalized} is available` : `${REASONS[avail()!.reason] ?? avail()!.reason}${avail()!.conflicting_name ? ` (${avail()!.conflicting_name})` : ""}`) : "3–32 characters. 1 HASH, valid about a year, 30-day grace period."}
            error={avail() && !avail()!.available && name().trim() ? " " : undefined}
          >
            <div class="flex items-center gap-2">
              <span class="text-muted">@</span>
              <Input mono value={name()} onInput={(e) => setName(e.currentTarget.value)} placeholder="alice" maxLength={32} />
              <Show when={avail()}>
                <Badge brand={avail()!.available} strong={!avail()!.available}>
                  {avail()!.available ? "available" : REASONS[avail()!.reason] ?? avail()!.reason}
                </Badge>
              </Show>
            </div>
          </Field>
          <div class="flex justify-end">
            <Button disabled={!registered() || !avail()?.available} onClick={() => setSpec({ type: "register_username", name: avail()!.normalized })}>
              <Plus size={14} /> Register @{avail()?.normalized || name()}
            </Button>
          </div>
        </div>
      </Card>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onDone={() => setName("")} />
    </div>
  );
}

function DevicesTab() {
  const [status, { refetch }] = createResource(
    () => store.ticks().wallet,
    () => ipc.identityStatus(),
  );
  const [add, setAdd] = createSignal(false);
  const [devId, setDevId] = createSignal("");
  const [pubkey, setPubkey] = createSignal("");
  const [label, setLabel] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const act = async (f: () => Promise<unknown>, done: string) => {
    setBusy(true);
    try {
      await f();
      store.toast(done);
      store.bump("wallet");
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
        {(s: () => IdentityStatus) => (
          <div class="flex max-w-3xl flex-col gap-4">
            <Show when={!s().this_device_registered}>
              <Card title="Register this identity">
                <div class="p-4">
                  <IdentityStep info={{ address: s().address, device_id: s().this_device_id, has_wallet_key: s().has_wallet_key, has_root_key: s().has_root_key }} onDone={() => void refetch()} embedded />
                </div>
              </Card>
            </Show>
            <Card
              title="Devices on chain"
              actions={
                <div class="flex gap-1">
                  <Button variant="ghost" size="sm" onClick={() => act(() => ipc.devicesReconcile(), "Groups reconciled with the chain")} loading={busy()}>
                    Reconcile
                  </Button>
                  <Button variant="secondary" size="sm" onClick={() => setAdd(true)} disabled={!s().has_root_key || !s().registered} title={s().has_root_key ? "" : "Only the device holding the root key adds devices"}>
                    <Plus size={12} /> Add device
                  </Button>
                </div>
              }
            >
              <table class="table">
                <thead>
                  <tr>
                    <th>Label</th>
                    <th>Device id</th>
                    <th>Public key</th>
                    <th>Platform</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  <For each={s().devices} fallback={<tr><td colSpan={5} class="text-center text-xs text-muted">{s().registered ? "No devices." : "The identity is not on chain yet."}</td></tr>}>
                    {(d) => (
                      <tr class={d.revoked ? "opacity-50" : ""}>
                        <td>
                          {d.label || "—"} {d.is_this_device ? <Badge brand>this PC</Badge> : null} {d.revoked ? <Badge>revoked</Badge> : null}
                        </td>
                        <td class="mono text-xs">{d.device_id}</td>
                        <td>
                          <Mono text={d.pubkey_hex} head={10} tail={6} copy />
                        </td>
                        <td class="text-xs text-muted">{d.platform}</td>
                        <td class="text-right">
                          <Show when={!d.revoked && !d.is_this_device}>
                            <Button variant="ghost" size="sm" loading={busy()} onClick={async () => { if (await confirm(`Revoke "${d.label || d.device_id}"? It stops receiving from the next encryption epoch; what it already holds it keeps.`)) await act(() => ipc.deviceRevoke(d.device_id), "Revocation submitted"); }}>
                              Revoke
                            </Button>
                          </Show>
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Card>
            <Card title="This device">
              <div class="flex flex-col gap-2 p-4 text-[13px]">
                <p>
                  Device id: <span class="mono">{s().this_device_id}</span>
                </p>
                <p class="flex items-center gap-2">
                  Public key: <Mono text={s().this_device_pubkey} full copy class="text-xs" />
                </p>
                <p class="text-xs text-muted">To add this PC from the device that created the identity, paste both values there under Add device. Then run Reconcile here once; the Drive keyring and contacts arrive at the next sync.</p>
                <div class="flex gap-2">
                  <Button variant="secondary" size="sm" onClick={() => void copyText(`${s().this_device_id}\n${s().this_device_pubkey}`)}>
                    <CopyIcon size={12} /> Copy id + key
                  </Button>
                  <Button variant="ghost" size="sm" loading={busy()} onClick={() => act(() => ipc.devicesBootstrap(), "Keyring and contacts sent to your other devices")}>
                    <Smartphone size={12} /> Send keyring to my other devices
                  </Button>
                </div>
              </div>
            </Card>
            <Dialog open={add()} onClose={() => setAdd(false)} title="Add a device" description="Paste the id and public key shown on the new device's Wallet → Devices page. Needs the root key (this device has it) and a little HASH for the fee." width="max-w-md">
              <div class="flex flex-col gap-3">
                <Field label="Device id">
                  <Input mono value={devId()} onInput={(e) => setDevId(e.currentTarget.value)} />
                </Field>
                <Field label="Public key (64 hex)">
                  <Input mono value={pubkey()} onInput={(e) => setPubkey(e.currentTarget.value)} />
                </Field>
                <Field label="Label">
                  <Input value={label()} onInput={(e) => setLabel(e.currentTarget.value)} placeholder="Laptop" />
                </Field>
                <div class="flex justify-end">
                  <Button
                    loading={busy()}
                    disabled={!devId().trim() || !/^[0-9a-fA-F]{64}$/.test(pubkey().trim())}
                    onClick={async () => {
                      await act(() => ipc.deviceAdd(devId().trim(), pubkey().trim(), label().trim()), "Device added; keyring sent");
                      setAdd(false);
                      setDevId(""); setPubkey(""); setLabel("");
                    }}
                  >
                    Add on chain
                  </Button>
                </div>
              </div>
            </Dialog>
          </div>
        )}
      </Show>
    </Show>
  );
}

function FounderTab() {
  const [params] = useChain(() => "hashgram/founder/v1/params");
  const [revenue] = useChain(() => "hashgram/founder/v1/revenue");
  const [history] = useChain(() => "hashgram/founder/v1/beneficiary_history");
  const [totals] = useChain(() => "hashgram/feerouter/v1/totals");
  const beneficiary = () => str(pick(params(), "params.beneficiary")) || str(pick(params(), "beneficiary"));
  const shareBps = () => str(pick(params(), "params.fee_basis_points") ?? pick(params(), "params.share_bps"), "100");
  const ceilingBps = () => str(pick(params(), "max_fee_basis_points") ?? pick(params(), "params.ceiling_bps"), "100");
  const rev = createMemo(() => ({
    accrued: coin(pick(revenue(), "total_accrued") ?? pick(revenue(), "accrued")),
    paid: coin(pick(revenue(), "total_paid") ?? pick(revenue(), "paid")),
    pending: coin(pick(revenue(), "pending")),
  }));
  const realised = createMemo(() => {
    const v = pick(totals(), "totals") ?? totals();
    const founder = BigInt(coin(pick(v, "founder_share")) || "0");
    const total = BigInt(coin(pick(v, "total_qualifying")) || "0");
    if (total === 0n) return null;
    return { bps: (Number((founder * 1_000_000n) / total) / 100).toFixed(2), founder, total };
  });
  return (
    <div class="flex flex-col gap-4">
      <Notice>
        The Founder receives a share of <strong>protocol fee revenue only</strong> — never of transferred principal. 100 HASH sent is 100 HASH received. The share is a hard-coded ceiling of 100 basis points (1 %).
      </Notice>
      <Show when={!params.loading} fallback={<Skeleton lines={3} />}>
        <Show when={params()} fallback={<Notice strong title="Founder data unavailable">{errText(params.error) || "No node answered."}</Notice>}>
          <div class="grid grid-cols-4 gap-3">
            <Stat label="Configured share" value={`${shareBps()} bps`} sub={`ceiling ${ceilingBps()} bps`} />
            <Stat label="Realised share" value={realised() ? `${realised()!.bps} bps` : "—"} sub={realised() ? `${formatHash(realised()!.founder)} / ${formatHash(realised()!.total)} HASH of qualifying fees` : "from feerouter totals"} />
            <Stat label="Accrued (lifetime)" value={`${formatHash(rev().accrued)} HASH`} />
            <Stat label="Paid / pending" value={`${formatHash(rev().paid)} HASH`} sub={`pending ${formatHash(rev().pending)} HASH`} />
          </div>
          <Card title="Beneficiary">
            <div class="flex flex-col gap-2 p-4 text-[13px]">
              <Mono text={beneficiary()} full copy />
              <p class="text-xs text-muted">Genesis allocation: 199,000,000 HASH — 19,000,000 spendable, 180,000,000 vesting over 96 months.</p>
            </div>
          </Card>
          <Card title="Beneficiary history">
            <Show when={arr(pick(history(), "history")).length} fallback={<p class="p-4 text-xs text-muted">The beneficiary has never changed.</p>}>
              <ul>
                <For each={arr(pick(history(), "history"))}>
                  {(h) => (
                    <li class="flex items-center justify-between border-b border-border px-4 py-2 text-[13px] last:border-0">
                      <Mono text={str(pick(h, "beneficiary"))} head={16} tail={6} />
                      <span class="mono text-xs text-muted">height {str(pick(h, "height"))}</span>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Card>
        </Show>
      </Show>
    </div>
  );
}