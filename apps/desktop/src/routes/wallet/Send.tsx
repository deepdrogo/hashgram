// Send: recipient by address or @username (resolved on chain, confusable
// warning), integer uhash, confirm dialog states transfers are untaxed.
import { createSignal, createResource, Show } from "solid-js";
import { Card, Field, Input, Button, Notice } from "~/components/ui";
import { Mono } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { ipc, type MsgSpec, type WalletOverview, type SearchResult } from "~/lib/ipc";
import { formatHash, isHashAddress, parseHashInput, toBig } from "~/lib/format";

export function Send(props: { overview: WalletOverview | null | undefined; onSent: () => void }) {
  const [to, setTo] = createSignal("");
  const [amount, setAmount] = createSignal("");
  const [memo, setMemo] = createSignal("");
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);

  const [resolved] = createResource(
    () => to().trim(),
    async (q): Promise<SearchResult | null> => {
      if (!q) return null;
      if (isHashAddress(q)) return { kind: "address", address: q, username: null };
      if (q.startsWith("@") && q.length >= 4) return ipc.searchResolve(q).catch(() => null);
      return null;
    },
  );
  const recipient = () => {
    const r = resolved();
    if (!r) return null;
    if (r.kind === "address") return r.address;
    if (r.kind === "username") return r.address;
    return null;
  };
  const uhash = () => parseHashInput(amount());
  const balance = () => toBig(props.overview?.balance_uhash);
  const tooMuch = () => (uhash() ?? 0n) > balance();
  const addrError = () => {
    const q = to().trim();
    if (!q) return undefined;
    if (q.startsWith("cosmos1")) return "cosmos1… is not a Hashgram address; Hashgram addresses start with hash1";
    if (q.startsWith("@")) {
      const r = resolved();
      if (resolved.loading) return undefined;
      if (r?.kind === "username_available") return "no such username on chain";
      if (r?.kind === "nothing") return r.reason;
      return undefined;
    }
    if (!isHashAddress(q)) return "not a valid hash1… address";
    return undefined;
  };
  const canSend = () => !!recipient() && (uhash() ?? 0n) > 0n && !tooMuch() && !addrError();

  return (
    <div class="grid grid-cols-[1fr_320px] gap-4">
      <Card title="Send HASH">
        <div class="flex flex-col gap-4 p-4">
          <Field label="Recipient" error={addrError()} hint="A hash1… address or an @username registered on chain.">
            <Input mono value={to()} onInput={(e) => setTo(e.currentTarget.value)} placeholder="hash1… or @name" aria-invalid={!!addrError()} />
          </Field>
          <Show when={resolved()?.kind === "username"}>
            {(_) => {
              const r = resolved() as Extract<SearchResult, { kind: "username" }>;
              return (
                <Notice title={`@${r.name} resolves to`}>
                  <Mono text={r.address} full class="text-xs" />
                </Notice>
              );
            }}
          </Show>
          <Field label="Amount (HASH)" error={amount() && uhash() === null ? "up to 6 decimals" : tooMuch() ? "more than your balance" : undefined} hint={uhash() !== null ? `${uhash()!.toString()} uhash` : undefined}>
            <div class="flex gap-2">
              <Input mono value={amount()} onInput={(e) => setAmount(e.currentTarget.value)} placeholder="0.000000" inputmode="decimal" />
              <Button variant="secondary" size="sm" class="h-9" onClick={() => setAmount(formatHash(balance(), 0, 6).replace(/,/g, ""))} disabled={balance() === 0n}>
                Max
              </Button>
            </div>
          </Field>
          <Field label="Memo (optional)" hint="Public on chain. Do not put secrets here.">
            <Input value={memo()} onInput={(e) => setMemo(e.currentTarget.value)} maxLength={256} />
          </Field>
          <div class="flex justify-end">
            <Button
              disabled={!canSend()}
              onClick={() => setSpec({ type: "send", to: recipient()!, amount_uhash: uhash()!.toString() })}
            >
              Review
            </Button>
          </div>
        </div>
      </Card>
      <div class="flex flex-col gap-3">
        <Notice title="Transfers are untaxed">
          100 HASH sent = 100 HASH received. The only cost is the network fee, shown before you confirm; 1 % of that fee — never of the amount — goes to the Founder.
        </Notice>
        <Notice title="Before you confirm">
          The sequence is fetched fresh, the transaction is signed on this PC and broadcast through the current chain source, then tracked by its hash until the chain includes it.
        </Notice>
      </div>
      <TxConfirm spec={spec()} memo={memo()} onClose={() => setSpec(null)} onSubmitted={() => { setAmount(""); setMemo(""); props.onSent(); }} />
    </div>
  );
}
