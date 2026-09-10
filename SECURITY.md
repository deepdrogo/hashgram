# Security policy

**Do not open a public issue for a security bug.** A consensus vulnerability
disclosed before a patch is deployed is a vulnerability being exploited.

Report privately through GitHub's **Report a vulnerability** button on this
repository (Security tab → Advisories). Include enough detail to reproduce:
version or commit, network (Mainnet / devnet), steps, and impact. Expect an
acknowledgement, a fix, and a coordinated disclosure once operators have had
time to upgrade.

In scope: `hashgramd` and the eight `x/` modules, `hashgram-node`,
`hashgram-sdk`, `hashgram-client`, `hashgramctl`, the indexer and safety
engine, the installer and systemd units, and the genesis/launch tooling.

Full policy, key-handling rules and hardening: [docs/SECURITY.md](docs/SECURITY.md).
Threat model: [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).

This repository contains no secrets. Keys, mnemonics and node identities are
generated on the machines that use them and are git-ignored; the history is
scanned by `gitleaks` in CI.
