# Your keys and what they control

## The 24 words

The 24-word phrase (256 bits, BIP-39) **is** your identity. From it the
app derives your **wallet key** and address `hash1…`, and your **identity
root key**, which authorises the devices that may speak for you. The same
24 words on a second PC give the same identity.

Anyone who has the 24 words has everything. Nobody can reset them: not the
network, not the Founder, not this app. There is no "forgot password".

## The passphrase

The passphrase encrypts the vault on this PC (Argon2id, 64 MiB memory
cost, then XChaCha20-Poly1305). It protects the keys at rest on this
machine only. Changing it re-encrypts the vault and disables Windows
Hello until re-enrolled.

## Windows Hello

When enabled, the passphrase is wrapped by Windows DPAPI with entropy
that exists only after a successful Hello prompt. Hello never sees the 24
words. Disabling Hello deletes the wrapped blob.

## Device keys

Each PC has its own **device key**, registered on chain. Mail, Drive
shares and Space events are authenticated by device keys through the
encryption groups; anyone can check them against the chain.

## Drive and mail keys

Every Drive file has its own key; the folder tree is encrypted with a
manifest key kept in the vault. Attachments carry their key inside the
encrypted message. None of these keys ever reach the user interface layer
of this app — they stay in the Rust core.

## Backup file

Settings → Security → **Export backup** writes an encrypted file
(Argon2id at 256 MiB, then XChaCha20-Poly1305) with the wallet key, root
key and Drive keyring — and deliberately **without** this device's key or
its encryption sessions, so a restored copy becomes a new device. The
host of the file cannot read it. Keep the backup passphrase separately.
