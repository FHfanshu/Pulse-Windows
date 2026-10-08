// Ported from upstream Sources/Pulse/Usage/UsageProvider.swift (first-20 subset).
//! The providers this port ships. Raw values match upstream so stored ids,
//! cache files and `--json` output stay compatible.

use serde::{Deserialize, Serialize};

macro_rules! providers {
    ($( $variant:ident => $raw:literal, $name:literal, $icon:literal, $multi:literal; )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum Provider {
            $( #[serde(rename = $raw)] $variant, )*
        }

        impl Provider {
            pub const ALL: &'static [Provider] = &[ $( Provider::$variant, )* ];

            pub fn raw(self) -> &'static str {
                match self { $( Provider::$variant => $raw, )* }
            }

            pub fn from_raw(raw: &str) -> Option<Self> {
                match raw { $( $raw => Some(Provider::$variant), )* _ => None }
            }

            /// Product name; never translated.
            pub fn display_name(self) -> &'static str {
                match self { $( Provider::$variant => $name, )* }
            }

            /// File stem in `assets/icons/`.
            pub fn icon_resource(self) -> &'static str {
                match self { $( Provider::$variant => $icon, )* }
            }

            /// Whether extra accounts may be added (upstream `supportsMultipleAccounts`).
            pub fn supports_multiple_accounts(self) -> bool {
                match self { $( Provider::$variant => $multi, )* }
            }
        }
    };
}

providers! {
    ClaudeCode   => "claudeCode",     "Claude Code",    "claude",     true;
    Codex        => "codex",          "Codex",          "openai",     true;
    Kiro         => "kiro",           "Kiro",           "kiro",       false;
    Antigravity  => "antigravity",    "Antigravity",    "antigravity", false;
    Cursor       => "cursor",         "Cursor",         "cursor",     false;
    OpenCodeGo   => "openCodeGo",     "OpenCode Go",    "opencode",   false;
    KimiCode     => "kimiCode",       "Kimi Code",      "kimi",       false;
    Zai          => "zai",            "Z.ai",           "zai",        false;
    Minimax      => "minimax",        "MiniMax",        "minimax",    false;
    Copilot      => "copilot",        "GitHub Copilot", "github",     false;
    Grok         => "grok",           "Grok",           "grok",       true;
    DeepSeek     => "deepSeek",       "DeepSeek",       "deepseek",   false;
    Factory      => "factory",        "Factory",        "extension",  false;
    Gemini       => "gemini",         "Gemini",         "geminicli",  false;
    Augment      => "augment",        "Augment Code",      "extension",  false;
    Warp         => "warp",           "Warp",           "extension",  false;
    Windsurf     => "windsurf",       "Windsurf",       "windsurf",   false;
    Moonshot     => "moonshot",       "Moonshot",       "moonshot",   false;
    OpenAiPlatform => "openAIPlatform", "OpenAI API",   "openai",     false;
    Amp          => "amp",            "Amp",            "amp",        false;
}

impl Provider {
    pub fn account_id(self) -> String {
        self.raw().to_string()
    }
}
