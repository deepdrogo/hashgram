import { Show } from "solid-js";
import { Card, Stat, Notice, Skeleton } from "~/components/ui";
import type { WalletOverview } from "~/lib/ipc";
import { formatHash, formatTime } from "~/lib/format";

export function Vesting(props: { overview: WalletOverview | null | undefined }) {
  const o = () => props.overview;
  const nextUnlock = () => {
    const end = o()?.vesting_end;
    if (!end) return null;
    const now = Math.floor(Date.now() / 1000);
    return end > now ? end : null;
  };
  return (
    <Show when={o()} fallback={<Skeleton lines={3} />}>
      <Show
        when={o()!.vesting_type}
        fallback={
          <Card>
            <div class="p-4">
              <Notice title="No vesting schedule">This account is a plain account: everything in it is spendable. Vesting accounts (the Founder's, for example) show their schedule here from the chain's account record.</Notice>
            </div>
          </Card>
        }
      >
        <div class="grid grid-cols-3 gap-3">
          <Stat label="Account type" mono={false} value={o()!.vesting_type!.split(".").pop() ?? ""} />
          <Stat label="Original vesting" value={`${formatHash(o()!.original_vesting_uhash ?? "0")} HASH`} />
          <Stat label={nextUnlock() ? "Fully vested on" : "Vesting complete"} value={formatTime(o()!.vesting_end)} sub={o()!.vesting_start ? `started ${formatTime(o()!.vesting_start)}` : undefined} />
        </div>
        <div class="mt-3">
          <Notice>Continuous vesting releases linearly between start and end. Delegating from a vesting account costs about 520k gas; the app allows for it when it estimates.</Notice>
        </div>
      </Show>
    </Show>
  );
}
