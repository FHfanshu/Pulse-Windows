// Ported from upstream Providers/UsagePages.swift: the provider's own page for an account's usage or billing,
// where one is known — what the dashboard's "Open <provider> usage page" goes to.
//
// Only pages somebody opens. A provider missing here gets no item rather than a guessed link. Keyed by the raw
// provider id; providers this port does not ship (upstream lists Ollama Cloud, Replicate, Mistral…) are left out
// until they are added.
const usagePages: Record<string, string> = {
  claudeCode: "https://claude.ai/settings/usage",
  codex: "https://chatgpt.com/codex/settings/usage",
  cursor: "https://cursor.com/dashboard?tab=usage",
  copilot: "https://github.com/settings/copilot",
  deepSeek: "https://platform.deepseek.com/usage",
  openAIPlatform: "https://platform.openai.com/usage",
  kimiCode: "https://www.kimi.com/code/console",
  amp: "https://ampcode.com/settings",
};

export const usagePage = (provider: string): string | null => usagePages[provider] ?? null;
