// Plain-text bodies are rendered as text nodes with http(s) URLs turned into
// anchors that open in the system browser. No HTML is ever interpreted here.

export interface TextPart {
  kind: "text" | "link";
  value: string;
}

const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]]/g;

export function linkify(text: string): TextPart[] {
  const out: TextPart[] = [];
  let last = 0;
  for (const m of text.matchAll(URL_RE)) {
    const i = m.index ?? 0;
    if (i > last) out.push({ kind: "text", value: text.slice(last, i) });
    out.push({ kind: "link", value: m[0] });
    last = i + m[0].length;
  }
  if (last < text.length) out.push({ kind: "text", value: text.slice(last) });
  return out;
}

/** The only HTML that ever reaches an iframe: the sender's body_html
 *  wrapped so nothing can load or run. The iframe itself has sandbox=""
 *  and a CSP meta that forbids scripts, remote images, forms and frames. */
export function sandboxDocument(bodyHtml: string, dark: boolean): string {
  const csp =
    "default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'; font-src 'none'; script-src 'none'; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";
  const fg = dark ? "#ffffff" : "#0d0d0d";
  const bg = dark ? "#000000" : "#ffffff";
  // Strip <script> and on* handlers defensively even though the CSP blocks them.
  const cleaned = bodyHtml
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<script[^>]*>/gi, "")
    .replace(/\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "")
    .replace(/<(iframe|object|embed|form|link|meta|base)[^>]*>[\s\S]*?<\/\1>/gi, "")
    .replace(/<(iframe|object|embed|form|link|meta|base)[^>]*\/?>/gi, "");
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>html,body{margin:0;padding:12px;background:${bg};color:${fg};font:13.5px/1.5 Inter,"Segoe UI",system-ui,sans-serif;word-break:break-word}a{color:inherit}img{max-width:100%}</style></head><body>${cleaned}</body></html>`;
}
