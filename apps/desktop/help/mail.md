# Mail

HashMail is end-to-end encrypted mail between Hashgram identities. A
message travels inside an encryption group that contains exactly the
sender's devices and the recipients' devices; store nodes hold ciphertext
for up to 30 days and see only a mailbox id and a size.

## Addresses

Three forms mean the same person: `hash1…` (the address), `@alice` (a
username registered on chain) and `alice@hashgram.io` (the mail form of
that username). The composer resolves what you type against the chain and
shows a chip with the name and address. A recipient must have opened
Hashgram once while online so a device key exists on chain; otherwise the
chip says so.

## Who sent this?

The reading pane shows the sender **as the network authenticated it**. A
message may claim any From line; if it does not match the authenticated
sender the app marks it and files it as spam.

## Requests

Mail from someone you have never written to lands in **Requests**. Accept
moves it to the Inbox; later mail from them goes straight there. Block
drops everything from that address.

## Attachments

Files up to 64 KiB ride inside the message; larger ones are encrypted and
stored on nodes, with the key inside the message. **Attach from Drive**
offers two modes: *Snapshot* sends the file as it is now; *Live* lets the
recipient follow your later edits until you revoke.

## Receipts

A *delivered* receipt is sent when a recipient's device decrypts the
message. A *read* receipt is sent only if you asked for one **and** the
reader allowed read receipts in Settings → Mail.

## External e-mail

Mail from the Internet arrives through a gateway that saw its plaintext;
it is marked **External — not end-to-end encrypted** with the gateway's
verdicts (SPF, DKIM, DMARC). Sending to an e-mail address needs a gateway
identity set in Settings → Network; the message goes to the gateway with
an `ext-to:` label and leaves the network in the clear from there.

## BCC

A BCC recipient gets a separate copy that the other recipients never see;
their copy shows a **BCC** chip and replies go to the sender only.
