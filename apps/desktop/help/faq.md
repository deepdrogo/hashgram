# FAQ

**Is there a sign-up?** No. An account is 24 words. There is no email,
phone number or password database anywhere in Hashgram.

**Can I use 12 words?** No. Hashgram accounts are 24 words only.

**Who can reset my passphrase?** Nobody. Restore from the 24 words.

**Where is my data?** On this PC under `%LOCALAPPDATA%\Hashgram\data`: an
encrypted vault, an encrypted database and a list of peers. Nothing is
uploaded anywhere except what you publish to the network.

**Which server does the app talk to?** None in particular. It talks to
nodes on the peer-to-peer network and compares what they say.

**Is there mining?** No. Nodes earn for storing and serving real bytes;
see *Earn by running a node*.

**Is the supply fixed?** Yes: 1,000,000,000 HASH, no mint module.

**Are transfers taxed?** No. 100 HASH sent is 100 HASH received. 1 % of
protocol *fees* goes to the Founder; the Founder screen shows it live.

**Can I buy or sell HASH in the app?** No. There is no exchange, price feed,
fiat or bridge in this application.

**Does the app send analytics?** No. The only outbound HTTPS is the signed
update check and any REST endpoint you configured yourself.

**Can I run a node?** Yes: **Earn → Run a node** installs one as a Windows
service managed by the app.

**What does "verified by 2 nodes" mean?** Two nodes run by different
operators returned byte-identical answers within three blocks of each
other. It does not mean a Merkle proof was checked.
