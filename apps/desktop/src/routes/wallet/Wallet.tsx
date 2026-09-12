import { Show, createResource } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Tabs, Skeleton } from "~/components/ui";
import { VerifiedBy } from "~/components/identity";
import { ipc } from "~/lib/ipc";
import { formatHash, formatUhash } from "~/lib/format";
import { Receive } from "./Receive";
import { Send } from "./Send";
import { History } from "./History";
import { Vesting } from "./Vesting";
import { Staking } from "./Staking";
import { Governance } from "./Governance";
import { Usernames } from "./Usernames";
import { Identity } from "./Identity";

const TABS = [
  { id: "receive", label: "Receive" },
  { id: "send", label: "Send" },
  { id: "history", label: "History" },
  { id: "vesting", label: "Vesting" },
  { id: "staking", label: "Staking" },
  { id: "governance", label: "Governance" },
  { id: "usernames", label: "Usernames" },
  { id: "identity", label: "Identity & devices" },
];

export function Wallet() {
  const location = useLocation();
  const navigate = useNavigate();
  const tab = () => location.pathname.split("/")[2] || "receive";
  const [overview, { refetch }] = createResource(() => ipc.walletOverview().catch(() => null));

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-end justify-between gap-4">
        <div>
          <h1 class="page-title">Wallet</h1>
          <Show when={overview()} fallback={<Skeleton class="mt-1 w-56" />}>
            {(o) => (
              <div class="mt-1 flex items-baseline gap-3">
                <span class="mono text-2xl font-semibold" title={formatUhash(o().balance_uhash)}>
                  {formatHash(o().balance_uhash)} <span class="text-base text-muted">HASH</span>
                </span>
                <VerifiedBy verification={o().verification} source={o().source} />
              </div>
            )}
          </Show>
        </div>
      </div>
      <Tabs tabs={TABS} value={tab()} onChange={(id) => navigate(`/wallet/${id}`)} />
      <div class="fade-in">
        <Show when={tab() === "receive"}>
          <Receive overview={overview()} />
        </Show>
        <Show when={tab() === "send"}>
          <Send overview={overview()} onSent={() => void refetch()} />
        </Show>
        <Show when={tab() === "history"}>
          <History address={overview()?.address} />
        </Show>
        <Show when={tab() === "vesting"}>
          <Vesting overview={overview()} />
        </Show>
        <Show when={tab() === "staking"}>
          <Staking address={overview()?.address} onChanged={() => void refetch()} />
        </Show>
        <Show when={tab() === "governance"}>
          <Governance />
        </Show>
        <Show when={tab() === "usernames"}>
          <Usernames overview={overview()} onChanged={() => void refetch()} />
        </Show>
        <Show when={tab() === "identity"}>
          <Identity />
        </Show>
      </div>
    </div>
  );
}
