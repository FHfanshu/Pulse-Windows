// Provider marks drawn as templates (currentColor), like upstream LobeIconView.
const raw = import.meta.glob("../../../assets/icons/*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

const icons: Record<string, string> = {};
for (const [path, svg] of Object.entries(raw)) {
  const name = path.split("/").pop()!.replace(/\.svg$/, "");
  icons[name] = svg
    .replace(/<title>[\s\S]*?<\/title>/, "")
    .replace(/\s(width|height)="[^"]*"/g, "")
    .replace(/\sstyle="[^"]*"/, "")
    .replace(/<svg/, '<svg width="100%" height="100%"');
}

/** File stem in assets/icons per provider raw value (upstream Provider.iconResource). */
const providerIcons: Record<string, string> = {
  claudeCode: "claude", codex: "openai", kiro: "kiro", antigravity: "antigravity", cursor: "cursor",
  openCodeGo: "opencode", kimiCode: "kimi", zai: "zai", minimax: "minimax", copilot: "github", grok: "grok",
  deepSeek: "deepseek", factory: "extension", gemini: "geminicli", augment: "extension", warp: "extension",
  windsurf: "windsurf", moonshot: "moonshot", openAIPlatform: "openai", amp: "amp",
};

export const providerNames: Record<string, string> = {
  claudeCode: "Claude Code", codex: "Codex", kiro: "Kiro", antigravity: "Antigravity", cursor: "Cursor",
  openCodeGo: "OpenCode Go", kimiCode: "Kimi Code", zai: "Z.ai", minimax: "MiniMax", copilot: "GitHub Copilot",
  grok: "Grok", deepSeek: "DeepSeek", factory: "Factory", gemini: "Gemini", augment: "Augment Code", warp: "Warp",
  windsurf: "Windsurf", moonshot: "Moonshot", openAIPlatform: "OpenAI API", amp: "Amp",
};

export function ProviderIcon({ provider, size, opacity = 1 }: { provider: string; size: number; opacity?: number }) {
  const svg = icons[providerIcons[provider] ?? "extension"] ?? icons.extension;
  return (
    <span
      aria-hidden
      style={{ width: size, height: size, display: "inline-flex", color: "currentColor", opacity }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
