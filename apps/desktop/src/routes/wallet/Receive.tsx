import { createResource, Show } from "solid-js";
import { Card, Button, Skeleton } from "~/components/ui";
import { Mono } from "~/components/identity";
import { ipc, type WalletOverview } from "~/lib/ipc";
import { copyText } from "~/lib/clipboard";

export function Receive(props: { overview: WalletOverview | null | undefined }) {
  const address = () => props.overview?.address ?? "";
  const link = () => (address() ? `hashgram://${address()}` : "");
  const [svg] = createResource(link, (l) => (l ? ipc.qrSvg(l) : Promise.resolve("")));
  return (
    <div class="grid grid-cols-[240px_1fr] gap-4">
      <Card class="flex items-center justify-center p-4">
        <Show when={svg()} fallback={<Skeleton class="h-48 w-48" />}>
          <div class="h-48 w-48 rounded-md bg-bg p-2" innerHTML={svg()!} aria-label="Address QR code" />
        </Show>
      </Card>
      <Card title="Your address">
        <div class="flex flex-col gap-4 p-4">
          <Show when={address()} fallback={<Skeleton />}>
            <Mono text={address()} full copy class="text-sm" />
          </Show>
          <Show when={props.overview?.username}>
            <p class="text-sm">
              Verified username: <span class="mono">@{props.overview!.username}</span>
            </p>
          </Show>
          <div class="flex gap-2">
            <Button variant="secondary" onClick={() => void copyText(address(), "Address copied")} disabled={!address()}>
              Copy address
            </Button>
            <Button variant="secondary" onClick={() => void copyText(link(), "Link copied")} disabled={!link()}>
              Share link
            </Button>
          </div>
          <p class="text-xs text-muted">
            Anyone can send HASH to this address or open your profile from the <span class="mono">hashgram://</span> link. Sharing an address is safe: it holds only public information.
          </p>
        </div>
      </Card>
    </div>
  );
}
