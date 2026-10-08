// What each service reads, shown before it is switched on (upstream `monitoringAccessDescription`).
// Values are upstream's English keys; the table is pure copy and never inspects credentials.

const apiKeyOnly = "Uses only the API key you enter in Settings. No Keychain prompt.";

export const accessDescriptions: Record<string, string> = {
  claudeCode: "Reads Claude Code's saved login from Keychain or its credentials file. May ask for Keychain access, including Claude Desktop's cookie storage.",
  codex: "Reads ~/.codex/auth.json and may run codex app-server with its saved login. Pulse does not request Keychain access.",
  kiro: "Runs Kiro CLI's native ACP usage method with its saved login. Pulse does not read or store Kiro credentials.",
  antigravity: "Reads the running editor's local language server and connection token. No Keychain prompt.",
  cursor: "Reads the login saved in Cursor's local database. No Keychain prompt.",
  grok: "Reads the login saved in ~/.grok/auth.json. No Keychain prompt.",
  openCodeGo: "Uses a key entered in Settings, or reads OpenCode's auth.json. No Keychain prompt.",
  copilot: "Uses the GitHub login you connect in Settings. No Keychain prompt.",
  gemini: "Reads the login saved in ~/.gemini/oauth_creds.json. No Keychain prompt.",
  windsurf: "Reads windsurf.com's sign-in from a Chromium browser when you press Read. No Keychain prompt.",
  augment: "Uses a browser session you import in Settings. Importing may ask for browser Keychain access.",
  kimiCode: apiKeyOnly,
  zai: apiKeyOnly,
  minimax: apiKeyOnly,
  deepSeek: apiKeyOnly,
  factory: apiKeyOnly,
  amp: apiKeyOnly,
  warp: apiKeyOnly,
  moonshot: apiKeyOnly,
  openAIPlatform: apiKeyOnly,
};
