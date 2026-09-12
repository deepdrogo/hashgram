# Feed, reels, channels

Everything public in Hashgram is a **signed social event**: a post, a
comment, a reaction, a follow, a profile update, a reel, a story, a channel
post. Events are signed by a device key that resolves on chain, so anyone
can verify who wrote what without trusting a server. This app verifies
every signature itself before showing an event; anything it cannot verify
is hidden and counted.

## Display names are not identity

A display name is just a signed event anyone can set to anything. Next to
every display name — on every message, post, comment and call screen — the
app shows the verified `@username` or the middle-truncated address, in
monospace. That is how you know who is speaking.

## Feed

A chronological feed of the accounts you follow. No ranking, no
recommendations, no "people you may know" — there is no server to compute
them. Mute and block are local to this app.

## Reels and stories

Reels are vertical videos (pre-encoded MP4 up to 100 MB), chunked into
1 MiB pieces and uploaded to `store`/`media` nodes; downloads verify every
chunk by hash and drop a provider that serves a bad one. Stories expire
after 24 hours.

## Channels

Broadcast channels are events too: create, join, post, moderate. Subscriber
counts come from the events the app has seen.

## Reporting

Safety verdicts come from nodes with the `safety` role and are readable
through the network. Reporting sends a signed report event; the app never
uploads your local mute/block lists.
