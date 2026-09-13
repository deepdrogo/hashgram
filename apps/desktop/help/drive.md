# Drive

HashDrive keeps your files encrypted on the network. Every file is
encrypted on this PC in 1 MiB segments with a fresh key; nodes store
ciphertext and never see a name, a folder or who shared what with whom.

## Where things are

The folder tree, names, versions and shares live in a **manifest** that
is itself encrypted with a key kept in your vault. Changes are local until
the next sync round publishes the manifest; the *changes will sync* badge
says when that is pending. Your other devices receive the manifest key
through the encrypted device channel.

## Versions

Overwriting a file keeps the previous content as a version (up to 50).
Open **Versions** to download or restore any of them.

## Trash

Trash is a flag; **Restore** clears it. **Delete** removes the entry for
good. Ciphertext already on nodes stays until it expires there — Drive
cannot order a node to delete, so the honest statement is: the key is
gone, the bytes may linger encrypted.

## Sharing

A share is a *capability*: the object key handed to the recipient inside
their encrypted channel. *Snapshot* shares a fixed version. *Live* pushes
every new version to the recipient until you **Revoke**. Revoking stops
future versions and re-keys the next one; bytes the recipient already
downloaded cannot be recalled.

**Shared with me** lists what others gave you, with the owner, the version
and whether it was updated or revoked. **Save to my Drive** copies the
reference into your own tree without re-uploading.

## Rekey

**Rekey** re-encrypts a file under a fresh key as a new version and
revokes its live shares. Use it after a device was lost.

## Limits in this version

Files up to 256 MiB. Larger files are refused with a message; a streaming
path is planned.
