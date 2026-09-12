# Your keys and what they control

## The 24 words

The 24-word phrase (256 bits of entropy, BIP-39) **is** your account. From
it the app derives:

- your **wallet key** (secp256k1, path `m/44'/118'/0'/0/0`) and address
  `hash1…`, which holds and sends HASH;
- your **identity root key**, which authorises the devices that may speak
  for you. It is derived deterministically from the wallet key, so the same
  24 words on a second PC give the same identity and that PC can add itself
  as a device.

Anyone who has the 24 words has everything. Nobody can reset them: not the
network, not the Founder, not this app. **Forgot passphrase** on this PC
means *restore from the 24 words*.

## The passphrase

The passphrase encrypts the vault on this PC (Argon2id, 64 MiB memory
cost, then XChaCha20-Poly1305). It protects the keys at rest on this
machine only. Changing it re-encrypts the vault; it does not change the
account.

## Windows Hello

When enabled, the passphrase is wrapped by Windows DPAPI with entropy that
exists only after a successful Hello prompt (a signature from the
Hello-protected key). Hello never sees the 24 words. Disabling Hello
deletes the wrapped blob.

## Device keys

Each PC has its own **device key** (Ed25519), registered on chain with
`MsgCreateIdentity` (first device) or `MsgAddDevice` (later ones). Messages
and posts are signed by device keys; anyone can check them against the
chain without asking a server. Revoke a lost device from
**Wallet → Identity & devices**.

Only public keys go on chain. Nothing here is ever uploaded: no cloud
backup, no email, no printing from the app, and the 24 words never go to
the clipboard.
