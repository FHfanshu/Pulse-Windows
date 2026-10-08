//! Every shipped `UsageService`. To add one: write `providers/<name>.rs`
//! implementing `UsageService`, declare it here, and add it to `registry()`.

pub mod profile;

pub mod amp;
pub mod augment;
pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod deepseek;
pub mod factory;
pub mod gemini;
pub mod grok;
pub mod kimi_code;
pub mod minimax;
pub mod moonshot;
pub mod openai_platform;
pub mod opencode_go;
pub mod warp;
pub mod windsurf;
pub mod zai;

use std::sync::Arc;

use crate::service::{Registry, UsageService};

pub fn registry() -> Registry {
    let services: Vec<Arc<dyn UsageService>> = vec![
        Arc::new(amp::Amp),
        Arc::new(augment::Augment),
        Arc::new(claude_code::ClaudeCode::default()),
        Arc::new(codex::Codex),
        Arc::new(cursor::Cursor),
        Arc::new(deepseek::DeepSeek::default()),
        Arc::new(factory::Factory),
        Arc::new(gemini::Gemini),
        Arc::new(grok::Grok),
        Arc::new(kimi_code::KimiCode),
        Arc::new(minimax::Minimax),
        Arc::new(moonshot::Moonshot::default()),
        Arc::new(openai_platform::OpenAiPlatform),
        Arc::new(opencode_go::OpenCodeGo),
        Arc::new(warp::Warp),
        Arc::new(windsurf::Windsurf),
        Arc::new(zai::Zai),
    ];
    services.into_iter().map(|s| (s.provider(), s)).collect()
}
