// History from /cosmos/tx/v1beta1/txs by sender/recipient events, merged
// with what this PC submitted, filters, CSV export.
import { createMemo, createResource, createSignal, For, Show } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { Card, Button, Input, Skeleton, Empty, Badge } from "~/components/ui";
import { Mono, VerifiedBy } from "~/components/identity";
import { VirtualList } from "~/components/VirtualList";
import { ipc, pick, str, arr, num } from "~/lib/ipc";
import { formatHash, formatIso, relTime } from "~/lib/format";
import { save } from "@tauri-apps/plugin-dialog";
import { store } from "~/lib/store";

interface Row {
  hash: string;
  height: number;
  time: string;
  kind: string;
  amount_uhash: string | null;
  counterparty: string | null;
  direction: "in" | "out" | "self" | "other";
  code: number;
  memo: string;
}

function rowsFromTxs(value: unknown, me: string): Row[] {
  const responses = arr(pick(value, "tx_responses"));
  const txs = arr(pick(value, "txs"));
  return responses.map((r, i) => {
    const tx = txs[i];
    const msgs = arr(pick(tx, "body.messages"));
    const first = msgs[0] ?? {};
    const type = str(pick(first, "@type"), "");
    let amount: string | null = null;
    let cp: string | null = null;
    let dir: Row["direction"] = "other";
    if (type.endsWith("MsgSend")) {
      const from = str(pick(first, "from_address"));
      const to = str(pick(first, "to_address"));
      const coin = arr(pick(first, "amount")).find((c) => str(pick(c, "denom")) === "uhash");
      amount = coin ? str(pick(coin, "amount"), "0") : "0";
      if (from === me && to === me) dir = "self";
      else if (from === me) {
        dir = "out";
        cp = to;
      } else {
        dir = "in";
        cp = from;
      }
    } else if (type.includes("staking") || type.includes("distribution")) {
      cp = str(pick(first, "validator_address")) || null;
      const a = pick(first, "amount.amount");
      amount = a ? str(a) : null;
      dir = "out";
    }
    return {
      hash: str(pick(r, "txhash")),
      height: num(pick(r, "height")),
      time: str(pick(r, "timestamp")),
      kind: type.split(".").pop()?.replace(/^Msg/, "") ?? "Tx",
      amount_uhash: amount,
      counterparty: cp,
      direction: dir,
      code: num(pick(r, "code")),
      memo: str(pick(tx, "body.memo")),
    };
  });
}

export function History(props: { address: string | undefined }) {
  const [params] = useSearchParams();
  const [filter, setFilter] = createSignal<"all" | "in" | "out">("all");
  const [q, setQ] = createSignal("");
  const [data] = createResource(
    () => props.address,
    async (me) => {
      const limit = 30;
      const queries = [`transfer.recipient='${me}'`, `message.sender='${me}'`];
      const paths = queries.map((qq) => `cosmos/tx/v1beta1/txs?query=${encodeURIComponent(qq)}&limit=${limit}&order_by=ORDER_BY_DESC`);
      const reads = await ipc.chainGetMany(paths);
      const rows = new Map<string, Row>();
      let verification = null;
      let source = null;
      const errors: string[] = [];
      for (const r of reads) {
        if ("Ok" in r) {
          verification = r.Ok.verification ?? verification;
          source = r.Ok.source;
          for (const row of rowsFromTxs(r.Ok.value, me)) rows.set(row.hash, row);
        } else errors.push(r.Err);
      }
      const list = [...rows.values()].sort((a, b) => b.height - a.height);
      return { list, verification, source, errors };
    },
  );
  const [local] = createResource(() => ipc.txRecent().catch(() => []));

  const filtered = createMemo(() => {
    const l = data()?.list ?? [];
    const f = filter();
    const s = q().trim().toLowerCase();
    return l.filter((r) => (f === "all" || r.direction === f) && (!s || r.hash.toLowerCase().includes(s) || (r.counterparty ?? "").includes(s) || r.memo.toLowerCase().includes(s)));
  });

  const exportCsv = async () => {
    const rows = filtered();
    const head = "hash,height,time,kind,direction,amount_uhash,amount_hash,counterparty,code,memo";
    const esc = (v: string) => `"${v.replace(/"/g, '""')}"`;
    const body = rows.map((r) => [r.hash, r.height, r.time, r.kind, r.direction, r.amount_uhash ?? "", r.amount_uhash ? formatHash(r.amount_uhash, 6, 6).replace(/,/g, "") : "", r.counterparty ?? "", r.code, esc(r.memo)].join(","));
    const csv = [head, ...body].join("\n");
    try {
      const path = await save({ defaultPath: "hashgram-history.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
      if (!path) return;
      await ipc.saveTextFile(path, csv);
      store.toast("CSV exported");
    } catch (e) {
      store.toast(String(e), "error");
    }
  };

  return (
    <div class="flex flex-col gap-3">
      <div class="flex items-center gap-2">
        <div class="flex gap-1">
          <For each={["all", "in", "out"] as const}>
            {(f) => (
              <Button size="sm" variant={filter() === f ? "primary" : "secondary"} onClick={() => setFilter(f)}>
                {f === "all" ? "All" : f === "in" ? "Received" : "Sent"}
              </Button>
            )}
          </For>
        </div>
        <Input class="max-w-xs" placeholder="Filter by hash, address, memo" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
        <span class="flex-1" />
        <Show when={data()}>
          <VerifiedBy verification={data()!.verification} source={data()!.source} />
        </Show>
        <Button size="sm" variant="secondary" onClick={exportCsv} disabled={!filtered().length}>
          Export CSV
        </Button>
      </div>
      <Show when={params.tx}>
        <Card class="p-3 text-xs">
          Looking at transaction <Mono text={String(params.tx)} head={16} tail={8} copy />
        </Card>
      </Show>
      <Show when={local()?.length}>
        <Card title="Submitted from this PC">
          <ul>
            <For each={local()!.slice(0, 5)}>
              {(r) => (
                <li class="flex items-center justify-between gap-3 border-b border-border px-4 py-2 text-sm last:border-0">
                  <span class="truncate">{r.summary}</span>
                  <span class="flex items-center gap-2">
                    <Badge strong={r.state === "pending"}>{r.state}</Badge>
                    <Mono text={r.hash} head={8} tail={6} copy class="text-xs" />
                    <span class="mono text-xs text-muted">{relTime(r.submitted)}</span>
                  </span>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>
      <Card class="h-[52vh]">
        <Show when={!data.loading} fallback={<div class="p-4"><Skeleton lines={6} /></div>}>
          <Show when={filtered().length} fallback={<Empty title="No transactions">{data()?.errors[0] ?? "Nothing on chain for this address yet."}</Empty>}>
            <VirtualList items={filtered()} estimateSize={52} key={(r) => r.hash}>
              {(r) => (
                <div class="flex items-center gap-3 border-b border-border px-4 py-2 text-sm row-hover">
                  <Badge strong={r.direction === "in"}>{r.direction === "in" ? "in" : r.direction === "out" ? "out" : r.kind}</Badge>
                  <span class="w-24 shrink-0">{r.kind}</span>
                  <span class="mono min-w-0 flex-1 truncate text-muted">{r.counterparty ? <Mono text={r.counterparty} /> : "—"}</span>
                  <span class="mono w-36 text-right">{r.amount_uhash !== null ? `${r.direction === "in" ? "+" : "−"}${formatHash(r.amount_uhash)} HASH` : ""}</span>
                  <span class="mono w-36 text-right text-xs text-muted" title={r.time}>
                    {formatIso(r.time)}
                  </span>
                  <Mono text={r.hash} head={6} tail={4} copy class="text-xs" />
                  <Show when={r.code !== 0}>
                    <Badge strong>failed</Badge>
                  </Show>
                </div>
              )}
            </VirtualList>
          </Show>
        </Show>
      </Card>
    </div>
  );
}
