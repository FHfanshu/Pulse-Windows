// Ported from upstream Usage/UsageProvider.swift (hasSourceChoice, soleRoute, usesAPIKey, usesServerAddress,
// reportsSpendableBalance, splitsByModelGroup, usesSessionCookie, readsBrowserStorage), Providers/ProviderAccess.swift
// (monitoringAccessDescription) and the profiles of the providers this port ships. The Rust core does not expose
// these, so they are a table here; add a row when a provider is registered.
import { t } from "../../shared/i18n";
import { accountId, type ProviderUsage } from "../../shared/model";
import type { AppSettings } from "../../shared/settings";

/** `t()` for a key that says "Mac": the Windows port says "PC". The other languages fall back to the original's text. */
export const pc = (key: string, ...args: (string | number)[]) => t(key.replace(/\bMac\b/g, "PC"), ...args);

export type UsageSourceId = "automatic" | "endpoint" | "tooling";
export type BrowserId = "firefox" | "chrome" | "edge" | "brave" | "vivaldi";
export type CredentialKind = "apiKey" | "sessionCookie" | "browserSession";

export interface ProviderMeta {
  /** Upstream `hasSourceChoice`: the route to the figures is a setting. */
  sourceChoice?: boolean;
  /** Pulse needs a pasted credential (`usesAPIKey`); what kind decides the row's title and the browser list. */
  credential?: CredentialKind;
  /** The text under the credential field (`keySubtitle`), as an upstream key (may say "Mac"). */
  keySubtitle?: string;
  /** `usesServerAddress`: somebody's own deployment. */
  serverAddress?: boolean;
  /** `soleRoute`: the one route, stated rather than offered. */
  soleRoute?: { name: string; note: string };
  /** A GitHub sign-in instead of a pasted key (Copilot). */
  githubSignIn?: boolean;
  /** `reportsSpendableBalance`: a "Warn below" figure is worth offering. */
  reportsBalance?: boolean;
  /** `billing == .api` and the ring is a balance's: the "Ring measures" row. */
  balanceRing?: boolean;
  /** `splitsByModelGroup`. */
  splitsByModelGroup?: boolean;
  /** `keepsLocalTranscripts`: the estimated value and the detailed card's history can be built. */
  keepsLocalTranscripts?: boolean;
  /** `monitoringAccessDescription` as an upstream key. */
  access: string;
  /** Docs/setup/<slug>.md */
  helpSlug: string;
}

const KEY_ONLY = "Uses only the API key you enter in Settings. No Keychain prompt.";

export const providerMeta: Record<string, ProviderMeta> = {
  claudeCode: {
    sourceChoice: true,
    keepsLocalTranscripts: true,
    access: "Reads Claude Code's saved login from its credentials file, and the Claude desktop app's usage record. No prompt.",
    helpSlug: "claude-code",
  },
  codex: {
    sourceChoice: true,
    keepsLocalTranscripts: true,
    access: "Reads ~/.codex/auth.json and may run codex app-server with its saved login. Pulse does not request Keychain access.",
    helpSlug: "codex",
  },
  kiro: {
    soleRoute: { name: "Kiro CLI ACP", note: "Uses Kiro CLI's signed-in session without reading its credentials." },
    access: "Runs Kiro CLI's native ACP usage method with its saved login. Pulse does not read or store Kiro credentials.",
    helpSlug: "kiro",
  },
  antigravity: {
    soleRoute: { name: "Antigravity's language server", note: "Only while Antigravity is open." },
    splitsByModelGroup: true,
    access: "Reads the running editor's local language server and connection token. No Keychain prompt.",
    helpSlug: "antigravity",
  },
  cursor: {
    soleRoute: { name: "Cursor's own login", note: "Uses the login Cursor already saved." },
    access: "Reads the login saved in Cursor's local database. No Keychain prompt.",
    helpSlug: "cursor",
  },
  grok: {
    soleRoute: { name: "Grok's own login", note: "Uses the login Grok's CLI already saved." },
    access: "Reads the login saved in ~/.grok/auth.json. No Keychain prompt.",
    helpSlug: "grok",
  },
  openCodeGo: {
    credential: "apiKey",
    access: "Uses a key entered in Settings, or reads OpenCode's auth.json. No Keychain prompt.",
    helpSlug: "opencode-go",
  },
  kimiCode: { credential: "apiKey", access: KEY_ONLY, helpSlug: "kimi-code" },
  zai: { credential: "apiKey", keySubtitle: "From z.ai. Stored encrypted on this Mac.", access: KEY_ONLY, helpSlug: "zai" },
  minimax: {
    credential: "apiKey",
    keySubtitle: "From platform.minimax.io. Stored encrypted on this Mac.",
    access: KEY_ONLY,
    helpSlug: "minimax",
  },
  copilot: {
    githubSignIn: true,
    access: "Uses the GitHub login you connect in Settings. No Keychain prompt.",
    helpSlug: "copilot",
  },
  deepSeek: {
    credential: "apiKey",
    keySubtitle: "From platform.deepseek.com. Stored encrypted on this Mac.",
    reportsBalance: true,
    balanceRing: true,
    access: KEY_ONLY,
    helpSlug: "deepseek",
  },
  gemini: {
    soleRoute: {
      name: "Gemini CLI's own login",
      note: "Uses the login Gemini CLI already saved. Using Gemini CLI renews it.",
    },
    access: "Reads the login saved in ~/.gemini/oauth_creds.json. No Keychain prompt.",
    helpSlug: "gemini",
  },
  windsurf: {
    credential: "browserSession",
    access: "Reads windsurf.com's sign-in from a Chromium browser when you press Read. No Keychain prompt.",
    helpSlug: "windsurf",
  },
  openAIPlatform: {
    credential: "apiKey",
    keySubtitle: "From platform.openai.com. Stored encrypted on this Mac.",
    reportsBalance: true,
    access: KEY_ONLY,
    helpSlug: "openai-api",
  },
  amp: {
    credential: "apiKey",
    keySubtitle: "From ampcode.com/settings. Stored encrypted on this Mac.",
    reportsBalance: true,
    access: KEY_ONLY,
    helpSlug: "amp",
  },
  augment: {
    credential: "sessionCookie",
    keySubtitle: "Copied from your browser. Stored encrypted on this Mac.",
    access: "Uses a browser session you import in Settings.",
    helpSlug: "augment",
  },
  factory: {
    credential: "apiKey",
    keySubtitle: "From app.factory.ai. Stored encrypted on this Mac.",
    access: KEY_ONLY,
    helpSlug: "factory",
  },
  warp: {
    credential: "apiKey",
    keySubtitle: "From Warp's settings, under API Keys. Stored encrypted on this Mac.",
    access: KEY_ONLY,
    helpSlug: "warp",
  },
  moonshot: {
    credential: "apiKey",
    keySubtitle: "From the Kimi Open Platform console, international or China. Stored encrypted on this Mac.",
    reportsBalance: true,
    access: KEY_ONLY,
    helpSlug: "moonshot",
  },
};

const FALLBACK: ProviderMeta = { access: "", helpSlug: "" };

export const metaOf = (provider: string): ProviderMeta => providerMeta[provider] ?? FALLBACK;

/** An account id's provider and whether it is the provider's first login. */
export function accountParts(id: string, settings: AppSettings): { provider: string; primary: boolean } {
  const extra = settings.extraAccounts.find((a) => a.id === id);
  if (extra) return { provider: extra.provider, primary: false };
  const [provider, slot] = id.split("#");
  return { provider, primary: !slot };
}

/** Whether the Connection card has anything to draw (upstream `hasConnectionControls`): first accounts only. */
export const hasConnectionControls = (provider: string, primary: boolean): boolean => {
  if (!primary) return false;
  const m = metaOf(provider);
  return !!(m.sourceChoice || m.githubSignIn || m.credential || m.soleRoute);
};

/** Upstream `ConnectionDiagnosticsView`'s remedy for an unavailability reason (`ConnectionRemedy.forReason`). */
export type Remedy =
  | { type: "signIn" | "editCredential" | "editAddress" | "readBrowser" | "connectStatusLine" | "retry" | "help" }
  | { type: "openApp"; name: string }
  | { type: "copyCommand"; command: string };

export function remedyFor(reason: string, primary: boolean): Remedy | null {
  if (!primary && ["signedOut", "claudeLoginExpired", "signInRequired", "grokLoginExpired", "cursorLoginExpired", "cursorSignInRequired"].includes(reason)) {
    return { type: "signIn" };
  }
  switch (reason) {
    case "loading":
    case "awaitingResponse":
      return null;
    case "notConnected": return { type: "connectStatusLine" };
    case "claudeSignInRequired":
    case "claudeLoginExpired": return { type: "copyCommand", command: "claude auth login" };
    case "signInRequired": return { type: "copyCommand", command: "codex login" };
    case "kiroSignInRequired": return { type: "copyCommand", command: "kiro-cli login" };
    case "grokSignInRequired":
    case "grokLoginExpired": return { type: "copyCommand", command: "grok" };
    case "claudeDesktopNotSignedIn":
    case "claudeDesktopSessionExpired": return { type: "openApp", name: "Claude" };
    case "cursorSignInRequired":
    case "cursorLoginExpired": return { type: "openApp", name: "Cursor" };
    case "antigravityNotRunning":
    case "antigravityNotAnswering": return { type: "openApp", name: "Antigravity" };
    case "notSignedIn":
    case "signedOut": return { type: "signIn" };
    case "apiKeyMissing":
    case "apiKeyRefused": return { type: "editCredential" };
    case "serverAddressMissing":
    case "serverAddressRefused": return { type: "editAddress" };
    case "sessionMissing":
    case "sessionExpired": return { type: "readBrowser" };
    case "unreachable":
    case "rateLimited":
    case "serverError":
    case "codexServerFailed": return { type: "retry" };
    default: return { type: "help" };
  }
}

export const helpUrl = (provider: string) =>
  `https://github.com/qunqin24/Pulse/blob/main/Docs/setup/${metaOf(provider).helpSlug || provider}.md`;

/** The reading for an account id, if the snapshot has one. */
export const usageOf = (usages: ProviderUsage[], id: string): ProviderUsage | undefined =>
  usages.find((u) => accountId(u.account) === id);
