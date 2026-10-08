//! Every shipped `UsageService`. To add one: write `providers/<name>.rs`
//! implementing `UsageService`, declare it here, and add it to `registry()`.

pub mod profile;

pub mod amp;
pub mod augment;
pub mod factory;
pub mod gemini;
pub mod moonshot;
pub mod openai_platform;
pub mod warp;
pub mod windsurf;

use std::sync::Arc;

use crate::service::{Registry, UsageService};

pub fn registry() -> Registry {
    let services: Vec<Arc<dyn UsageService>> = vec![
        Arc::new(moonshot::Moonshot::default()),
    ];
    services.into_iter().map(|s| (s.provider(), s)).collect()
}
