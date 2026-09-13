// Formatting. Amounts are strings of uhash; never a float in arithmetic.
// `formatHashSdk` mirrors hashgram_sdk::wallet::format_hash exactly
// (tests/format.test.ts checks parity); `formatHash` is the UI variant with
// thousands separators and trimmed decimals.

export const UHASH_PER_HASH = 1_000_000n;

export function toBig(uhash: string | number | bigint | null | undefined): bigint {
  if (uhash === null || uhash === undefined || uhash === "") return 0n;
  try {
    return BigInt(typeof uhash === "string" ? uhash.trim() : uhash);
  } catch {
    return 0n;
  }
}

/** Exactly `hashgram_sdk::wallet::format_hash`: "x.yyyyyy HASH". */
export function formatHashSdk(uhash: string | bigint | number): string {
  const v = toBig(uhash);
  return `${v / UHASH_PER_HASH}.${(v % UHASH_PER_HASH).toString().padStart(6, "0")} HASH`;
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

/** Exactly `hashgram_sdk::wallet::parse_amount`: "1.5" / "1.5 HASH" /
 *  "1500000uhash" → uhash; null when invalid. */
export function parseAmount(input: string): bigint | null {
  let t = input.trim().toLowerCase();
  if (!t) return null;
  if (t.endsWith("uhash")) {
    const u = t.slice(0, -5).trim();
    return /^\d+$/.test(u) ? BigInt(u) : null;
  }
  if (t.endsWith("hash")) t = t.slice(0, -4).trim();
  const [whole = "", frac = ""] = t.split(".", 2);
  if (t.split(".").length > 2) return null;
  if (frac.length > 6 || (whole === "" && frac === "")) return null;
  if (whole !== "" && !/^\d+$/.test(whole)) return null;
  if (frac !== "" && !/^\d+$/.test(frac)) return null;
  return BigInt(whole || "0") * UHASH_PER_HASH + BigInt((frac + "000000").slice(0, 6));
}

export function truncateMiddle(s: string, head = 10, tail = 6): string {
  if (!s || s.length <= head + tail + 1) return s ?? "";
  return `${s.slice(0, head)}…${s.slice(-tail)}`;
}

/** A short handle for an address: @name when known, else hash1abc…wxyz. */
export function handle(address: string, username?: string | null, displayName?: string | null): string {
  if (displayName && displayName.trim()) return displayName.trim();
  if (username && username.trim()) return `@${username.trim()}`;
  return truncateMiddle(address, 9, 4);
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

export function isHex(s: string, bytes?: number): boolean {
  const t = s.trim();
  if (!/^[0-9a-fA-F]*$/.test(t) || t.length % 2) return false;
  return bytes ? t.length === bytes * 2 : t.length > 0;
}

const dateFmt = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit" });
const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "2-digit" });
const yearFmt = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "2-digit" });

export function formatTime(unixSecs: number | string | null | undefined): string {
  if (!unixSecs) return "—";
  const n = typeof unixSecs === "string" ? Number(unixSecs) : unixSecs;
  if (!Number.isFinite(n) || n <= 0) return "—";
  return dateFmt.format(new Date(n * 1000));
}

export function formatMs(ms: number | null | undefined): string {
  if (!ms) return "—";
  return dateFmt.format(new Date(ms));
}

/** Mail-list style: time today, day this year, date otherwise. */
export function shortWhen(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const now = new Date();
  if (d.toDateString() === now.toDateString()) return timeFmt.format(d);
  if (d.getFullYear() === now.getFullYear()) return dayFmt.format(d);
  return yearFmt.format(d);
}

export function formatIso(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return dateFmt.format(d);
}

export function relTime(unixSecs: number): string {
  const d = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (d < 5) return "just now";
  if (d < 60) return `${d} s ago`;
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
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

export function roleName(role: number): string {
  return ["—", "Guest", "Member", "Admin", "Owner"][role] ?? "—";
}

/** Splits comma/semicolon/space-separated recipients as typed. */
export function splitRecipients(s: string): string[] {
  return s
    .split(/[,;\n]+|\s{2,}/)
    .map((x) => x.trim())
    .filter(Boolean);
}
