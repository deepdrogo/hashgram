# Messages and devices

Messages are end-to-end encrypted with **MLS** (RFC 9420). Nodes with the
`store` role hold encrypted envelopes in mailboxes until your devices fetch
them (store-and-forward), so a recipient who is offline gets the message
when they return. No node can read a message.

## What the protocol provides

- Direct and group chats; text, replies, reactions.
- Attachments: encrypted, chunked, hash-verified, uploaded to `store`/`media`
  nodes; resumable.
- Voice notes.
- Delivery and read receipts (sent inside the encrypted channel).
- Disappearing messages: a timer carried in the message and honoured by
  every device; nodes cannot enforce it and never see it.
- Multi-device: every registered device receives every message; each
  message shows which device sent it.

## What it does not provide

- Push notifications: the app stays connected or polls.
- Typing indicators beyond a best-effort signal between online devices.

## Key packages

On first run each device publishes key packages to store nodes so others
can start an encrypted conversation with it. If someone cannot reach you,
they may be seeing "no key package found": open the app once so it can
publish.

## Chat info

Each chat lists the store nodes currently holding its mailbox and the
devices of every participant, all resolved from the chain.
