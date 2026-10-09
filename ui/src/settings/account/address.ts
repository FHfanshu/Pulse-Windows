// Ported from upstream Providers/GatewayAddress.swift (`url(from:path:)`, `isUsable`, `allowsPlainHTTP`): what makes a
// typed server address usable. https always; http only for a machine on the reader's own network.

/** Plain http is for a server on the reader's own network: loopback, private ranges, link-local, `.local` names. */
export function allowsPlainHTTP(host: string): boolean {
  const name = host.toLowerCase().replace(/^\[|\]$/g, "");
  if (name === "localhost" || name.endsWith(".localhost")) return true;
  if (name === "::1") return true;
  if (name.endsWith(".local")) return true;
  // Unique-local (fc00::/7) and link-local (fe80::/10). The colon test keeps a *name* beginning "fd" out of this.
  if (name.includes(":") && (name.startsWith("fc") || name.startsWith("fd") || name.startsWith("fe80:"))) return true;

  const octets = name.split(".");
  if (octets.length !== 4 || !octets.every((o) => /^\d{1,3}$/.test(o) && Number(o) <= 255)) return false;
  const [a, b] = octets.map(Number);
  return a === 127 || a === 10 || (a === 192 && b === 168) || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31);
}

export function isUsableAddress(typed: string): boolean {
  const trimmed = typed.trim();
  if (!trimmed) return false;
  let url: URL;
  try {
    url = new URL(trimmed.includes("://") ? trimmed : `https://${trimmed}`);
  } catch {
    return false;
  }
  const scheme = url.protocol.replace(":", "");
  if (scheme !== "https" && scheme !== "http") return false;
  if (!url.hostname) return false;
  if (url.username || url.password) return false;
  if (url.hash || url.search) return false;
  return scheme === "https" || allowsPlainHTTP(url.hostname);
}
