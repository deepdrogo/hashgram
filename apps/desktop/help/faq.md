# FAQ

**Is there a server?** No. The app talks to a network of nodes run by
independent operators and checks what they say against each other and
against signatures. Any one node can disappear.

**Can a node read my mail or files?** No. Everything is encrypted on
your PC before it leaves; nodes store ciphertext and see sizes and
times.

**Why do I need HASH to start?** Registering your identity and this
device's key on chain is one transaction with a fee. Until then the app
works locally (Drive, drafts, settings) and shows your address so someone
can send you HASH. There is no faucet on Mainnet today.

**Is there mining?** No. Nodes earn from proven work — storage
challenges and signed receipts — paid from a fixed reserve.

**What if I lose my PC?** Restore from the 24 words (or a backup file)
on another PC, then revoke the lost device from Wallet → Devices. The
lost device stops receiving from the next encryption epoch on; what it
already held it keeps.

**Why is a sender shown differently from the From line?** The network
authenticates the sender's device; the From line is only a claim. The
authenticated name is what you see.

**Can I e-mail people outside Hashgram?** Through a gateway, yes; such
mail is not end-to-end encrypted and is labelled. Set the gateway's
identity in Settings → Network.

**Why chronological only?** Because ranking is a decision someone else
makes for you. Friends and Following are in time order; Explore is the
indexer's chronological feed.

**Where is my data?** `%LOCALAPPDATA%\Hashgram\data`. Open it from
Settings → About.
