// Formatting. Amounts are strings of uhash; never a float in arithmetic.

export const UHASH_PER_HASH = 1_000_000n;

export function toBig(uhash: string | number | bigint | null | undefined): bigint {
  if (uhash === null || uhash === undefined || uhash === "") return 0n;
  try {
    return BigInt(typeof uhash === "string" ? uhash.trim() : uhash);
  } catch {
    return 0n;
  }
}

/** "1,234.56" style; trims trailing zeros to at least `minFrac`. */
export function formatHash(uhash: string | bigint | number | null | undefined, minFrac = 2, maxFrac = 6): string {
  const v = toBig(uhash);
  const neg = v < 0n;
  const abs = neg ? -v : v;
  const whole = abs / UHASH_PER_HASH;
  let frac = (abs % UHASH_PER_HASH).toString().padStart(6, "0").slice(0, maxFrac);
  while (frac.length > minFrac && frac.endsWith("0")) frac = frac.slice(0, -1);
  const wholeStr = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${neg ? "-" : ""}${wholeStr}${frac.length ? "." + frac : ""}`;
}

/** Formats uhash with thousands separators. */
export function formatUhash(uhash: string | bigint | number | null | undefined): string {
  return toBig(uhash).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",") + " uhash";
}

/** Parses a typed HASH amount into uhash; null when invalid. */
export function parseHashInput(input: string): bigint | null {
  const t = input.trim().replace(/[,_\s]/g, "");
  if (!t) return null;
  if (!/^\d*(\.\d{0,6})?$/.test(t) || t === ".") return null;
  const [w = "0", f = ""] = t.split(".");
  return BigInt(w || "0") * UHASH_PER_HASH + BigInt((f + "000000").slice(0, 6));
}

export function truncateMiddle(s: string, head = 10, tail = 6): string {
  if (!s || s.length <= head + tail + 1) return s ?? "";
  return `${s.slice(0, head)}…${s.slice(-tail)}`;
}

export function isHashAddress(s: string): boolean {
  return /^hash1[02-9ac-hj-np-z]{38}$/.test(s.trim());
}

export function isValoper(s: string): boolean {
  return /^hashvaloper1[02-9ac-hj-np-z]{38}$/.test(s.trim());
}

export function isTxHash(s: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(s.trim());
}

export function formatTime(unixSecs: number | string | null | undefined): string {
  if (!unixSecs) return "—";
  const n = typeof unixSecs === "string" ? Number(unixSecs) : unixSecs;
  if (!Number.isFinite(n) || n <= 0) return "—";
  return new Date(n * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatIso(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function relTime(unixSecs: number): string {
  const d = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (d < 60) return `${d}s ago`;
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86400)}d ago`;
}

export function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  return `${(n / 1024 ** 3).toFixed(2)} GiB`;
}

/** Basis points to a percentage string. */
export function bps(v: number | string): string {
  const n = typeof v === "string" ? Number(v) : v;
  return `${(n / 100).toFixed(2)} %`;
}

/** Blocks to a rough duration at ~4 s per block. */
export function blocksToDuration(blocks: number): string {
  return formatDuration(Math.max(0, Math.round(blocks * 4)));
}

export function pct(part: bigint, whole: bigint): string {
  if (whole === 0n) return "0.00 %";
  const p = Number((part * 10000n) / whole) / 100;
  return `${p.toFixed(2)} %`;
}

export function sourceLabel(s: { kind: string; url?: string } | null | undefined): string {
  if (!s) return "no source";
  if (s.kind === "local_node") return "node on this PC";
  if (s.kind === "p2p_relay") return "P2P relay";
  if (s.kind === "https") return s.url ?? "HTTPS";
  return s.kind;
}

/** The sentence under a balance. */
export function verificationLabel(v: { agreed: boolean; single_operator: boolean; peers: string[] } | null | undefined, source?: { kind: string } | null): string {
  if (!v) {
    if (source?.kind === "local_node") return "from the node on this PC";
    if (source?.kind === "https") return "from a REST endpoint you configured";
    return "not verified";
  }
  if (v.agreed) return `verified by ${v.peers.length} nodes`;
  if (v.single_operator) return "verified by 1 node · single operator on network";
  return `verified by ${v.peers.length} node · no second operator answered`;
}
