//! Keeps the model registry's live layer fresh: model names, context
//! windows and prices from the community LiteLLM registry, cached on disk
//! and refreshed once a day in the background.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use larrier_core::models::{self, ModelInfo};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: DateTime<Utc>,
    models: Vec<ModelInfo>,
}

/// When the live layer was last fetched, and whether a refresh is running.
#[derive(Clone, Default, PartialEq)]
pub struct Status {
    pub fetched_at: Option<DateTime<Utc>>,
    pub refreshing: bool,
    pub error: Option<String>,
}

static STATUS: Mutex<Status> = Mutex::new(Status { fetched_at: None, refreshing: false, error: None });

pub fn status() -> Status {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn cache_path() -> PathBuf {
    crate::state::store().root().join("cache").join("models.json")
}

/// Load the cache now; refresh in the background when it is a day old.
pub fn start() {
    let mut stale = true;
    if let Ok(t) = std::fs::read_to_string(cache_path()) {
        if let Ok(c) = serde_json::from_str::<Cached>(&t) {
            stale = Utc::now() - c.fetched_at > chrono::Duration::hours(24);
            if let Ok(mut s) = STATUS.lock() {
                s.fetched_at = Some(c.fetched_at);
            }
            models::set_live(c.models);
        }
    }
    if stale {
        std::thread::spawn(|| {
            // Let the window come up first.
            std::thread::sleep(Duration::from_secs(4));
            let _ = refresh_blocking();
        });
    }
}

/// Fetch, parse, install and cache the registry. Returns the model count.
pub fn refresh_blocking() -> anyhow::Result<usize> {
    {
        let mut s = STATUS.lock().map_err(|_| anyhow::anyhow!("status lock"))?;
        if s.refreshing {
            return Ok(models::live_count());
        }
        s.refreshing = true;
        s.error = None;
    }
    let result = (|| -> anyhow::Result<usize> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let text = rt.block_on(async {
            let client = reqwest::Client::builder().timeout(Duration::from_secs(45)).user_agent("Larrier").build()?;
            let resp = client.get(models::LITELLM_URL).send().await?.error_for_status()?;
            anyhow::Ok(resp.text().await?)
        })?;
        let list = models::parse_litellm(&text)?;
        let n = list.len();
        let cached = Cached { fetched_at: Utc::now(), models: list.clone() };
        models::set_live(list);
        let path = cache_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&cached)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(n)
    })();
    if let Ok(mut s) = STATUS.lock() {
        s.refreshing = false;
        match &result {
            Ok(_) => s.fetched_at = Some(Utc::now()),
            Err(e) => {
                tracing::warn!("model registry refresh failed: {e}");
                s.error = Some(e.to_string());
            }
        }
    }
    result
}
