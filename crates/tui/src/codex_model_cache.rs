//! Account-scoped roster for official Sign in with ChatGPT plan use.
//!
//! This replaces external Codex CLI cache/app-server discovery. Network access
//! uses the existing Codewhale provider client; only secret-free model metadata
//! is cached, keyed by the verified issuer, issued client ID, and subject.
//! A missing account roster offers no models. Public catalog rows and legacy
//! Codex credentials never prove permission to use a ChatGPT plan.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{ApiProvider, Config};

const MAX_MODEL_CACHE_BYTES: u64 = 4 * 1024 * 1024;
const MODEL_CACHE_MAX_AGE: Duration = Duration::hours(24);
const MAX_FUTURE_CLOCK_SKEW: Duration = Duration::minutes(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexModelCacheFreshness {
    Fresh,
    Missing,
    Stale,
    Invalid,
}

impl CodexModelCacheFreshness {
    #[must_use]
    pub(crate) const fn picker_label(self) -> &'static str {
        match self {
            Self::Fresh => "ChatGPT OAuth",
            Self::Missing => "OAuth roster missing",
            Self::Stale => "OAuth roster stale",
            Self::Invalid => "OAuth roster invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexModelRoster {
    pub(crate) models: Vec<CodexModelMetadata>,
    pub(crate) freshness: CodexModelCacheFreshness,
    pub(crate) fetched_at: Option<DateTime<Utc>>,
    pub(crate) observed_at: Option<DateTime<Utc>>,
    pub(crate) source: &'static str,
    pub(crate) observation_persisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodexModelMetadata {
    pub(crate) id: String,
    pub(crate) context_window: Option<u32>,
    pub(crate) reasoning: Option<bool>,
    pub(crate) efforts: Vec<String>,
}

impl CodexModelRoster {
    fn fallback(freshness: CodexModelCacheFreshness, fetched_at: Option<DateTime<Utc>>) -> Self {
        Self {
            models: Vec::new(),
            freshness,
            fetched_at,
            observed_at: None,
            source: "chatgpt_plan_api",
            observation_persisted: false,
        }
    }

    #[must_use]
    pub(crate) fn model_ids(&self) -> Vec<String> {
        self.models.iter().map(|model| model.id.clone()).collect()
    }

    #[must_use]
    pub(crate) fn metadata_for(&self, id: &str) -> Option<&CodexModelMetadata> {
        self.models.iter().find(|model| model.id.eq_ignore_ascii_case(id.trim()))
    }

    #[must_use]
    pub(crate) fn preferred_model_id(&self) -> Option<&str> {
        (self.freshness == CodexModelCacheFreshness::Fresh)
            .then(|| self.models.first().map(|model| model.id.as_str()))
            .flatten()
    }
}

#[derive(Serialize, Deserialize)]
struct CatalogSnapshot {
    fetched_at: DateTime<Utc>,
    models: Vec<CodexModelMetadata>,
}

type RosterCacheKey = (PathBuf, Option<SystemTime>, u64);
static ROSTER_MEMO: Mutex<Option<(RosterCacheKey, CodexModelRoster)>> = Mutex::new(None);

/// An unscoped completion/catalog cannot borrow another account's roster.
#[must_use]
pub(crate) fn model_roster() -> CodexModelRoster {
    CodexModelRoster::fallback(CodexModelCacheFreshness::Missing, None)
}

#[must_use]
pub(crate) fn model_roster_for(config: &Config) -> CodexModelRoster {
    let Some(path) = snapshot_path(config) else {
        return model_roster();
    };
    let key = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => (path.clone(), metadata.modified().ok(), metadata.len()),
        Err(_) => (path.clone(), None, 0),
    };
    let now = Utc::now();
    if let Ok(memo) = ROSTER_MEMO.lock()
        && let Some((cached_key, roster)) = memo.as_ref()
        && *cached_key == key
        && roster.freshness == CodexModelCacheFreshness::Fresh
        && roster.fetched_at.is_some_and(|fetched| now.signed_duration_since(fetched) <= MODEL_CACHE_MAX_AGE)
    {
        return roster.clone();
    }
    let roster = load_snapshot(&path, now);
    if let Ok(mut memo) = ROSTER_MEMO.lock() {
        *memo = Some((key, roster.clone()));
    }
    roster
}

fn registration_key(registration: &crate::oauth::ChatgptRegistration) -> String {
    let mut identity = Sha256::new();
    identity.update(b"codewhale-chatgpt-plan-roster-v1\0");
    for value in [&registration.issuer, &registration.client_id, &registration.subject] {
        identity.update(value.len().to_le_bytes());
        identity.update(value.as_bytes());
    }
    identity.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn snapshot_path(config: &Config) -> Option<PathBuf> {
    if config.provider_uses_custom_endpoint(ApiProvider::OpenaiCodex) {
        return None;
    }
    let registration = crate::oauth::official_chatgpt_registration(config).ok()?;
    let catalog_path = crate::models_dev_live::cache_path()?;
    Some(catalog_path.parent()?.join(format!("chatgpt-plan-{}.json", registration_key(&registration))))
}

fn load_snapshot(path: &Path, now: DateTime<Utc>) -> CodexModelRoster {
    let bytes = match read_cache_bytes(path) {
        Ok(bytes) => bytes,
        Err(freshness) => return CodexModelRoster::fallback(freshness, None),
    };
    let snapshot: CatalogSnapshot = match serde_json::from_slice(&bytes) {
        Ok(snapshot) => snapshot,
        Err(_) => return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, None),
    };
    let age = now.signed_duration_since(snapshot.fetched_at);
    if age < -MAX_FUTURE_CLOCK_SKEW || snapshot.models.iter().any(|model| {
        !crate::provider_lake::valid_catalog_model_id(&model.id)
            || model.efforts.len() > 16
            || model.efforts.iter().any(|effort| !valid_effort(effort))
            || model.context_window.is_some_and(|window| !(1..=16_000_000).contains(&window))
    }) {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Invalid, Some(snapshot.fetched_at));
    }
    if age > MODEL_CACHE_MAX_AGE {
        return CodexModelRoster::fallback(CodexModelCacheFreshness::Stale, Some(snapshot.fetched_at));
    }
    CodexModelRoster {
        models: snapshot.models,
        freshness: CodexModelCacheFreshness::Fresh,
        fetched_at: Some(snapshot.fetched_at),
        observed_at: None,
        source: "chatgpt_plan_api",
        observation_persisted: true,
    }
}

fn valid_effort(effort: &str) -> bool {
    !effort.is_empty() && effort.len() <= 32
        && effort.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Fetch through the existing provider client. A registration change during
/// the request cannot publish an old account's models under a new account.
pub(crate) async fn update_from_chatgpt(config: &Config) -> Result<CodexModelRoster, &'static str> {
    let config = config.clone();
    let prepared = config.clone();
    let (path, client) = tokio::task::spawn_blocking(move || {
        let path = snapshot_path(&prepared).ok_or("chatgpt_plan_permission_required")?;
        let client = crate::client::CodewhaleClient::for_catalog_refresh(&prepared)
            .map_err(|_| "chatgpt_plan_credentials_unavailable")?;
        Ok::<_, &'static str>((path, client))
    }).await.map_err(|_| "chatgpt_plan_credentials_unavailable")??;
    let delta = tokio::time::timeout(std::time::Duration::from_secs(20), client.fetch_catalog_delta())
        .await.map_err(|_| "chatgpt_models_timeout")?
        .map_err(|error| match error {
            codewhale_config::catalog::CatalogRefreshError::Unauthorized => "chatgpt_models_unauthorized",
            codewhale_config::catalog::CatalogRefreshError::Forbidden => "chatgpt_models_forbidden",
            codewhale_config::catalog::CatalogRefreshError::RateLimited => "chatgpt_models_usage_limit",
            _ => "chatgpt_models_unavailable",
        })?;
    let models = delta.offerings.into_iter().map(|row| CodexModelMetadata {
        id: row.wire_model_id,
        context_window: row.limit.and_then(|limit| limit.context).and_then(|value| u32::try_from(value).ok()),
        reasoning: row.reasoning,
        efforts: row.reasoning_options.into_iter().filter_map(|value| value.as_str().map(str::to_string)).collect(),
    }).collect();
    let snapshot = CatalogSnapshot { fetched_at: Utc::now(), models };
    tokio::task::spawn_blocking(move || {
        if snapshot_path(&config).as_ref() != Some(&path) {
            return Err("refresh_credentials_changed");
        }
        let encoded = serde_json::to_vec(&snapshot).map_err(|_| "cache_write_failed")?;
        if encoded.len() as u64 > MAX_MODEL_CACHE_BYTES {
            return Err("chatgpt_models_response_too_large");
        }
        codewhale_config::persistence::atomic_write(&path, &encoded).map_err(|_| "cache_write_failed")?;
        if let Ok(mut memo) = ROSTER_MEMO.lock() {
            *memo = None;
        }
        Ok(load_snapshot(&path, Utc::now()))
    }).await.map_err(|_| "cache_write_failed")?
}

fn read_cache_bytes(path: &Path) -> Result<Vec<u8>, CodexModelCacheFreshness> {
    let path_metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CodexModelCacheFreshness::Missing);
        }
        Err(_) => return Err(CodexModelCacheFreshness::Invalid),
    };
    if !path_metadata.file_type().is_file() || path_metadata.len() > MAX_MODEL_CACHE_BYTES {
        return Err(CodexModelCacheFreshness::Invalid);
    }
    let mut file = match open_cache_file(path) {
        Ok(file) => file,
        Err(_) => return Err(CodexModelCacheFreshness::Invalid),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Err(CodexModelCacheFreshness::Invalid),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_MODEL_CACHE_BYTES {
        return Err(CodexModelCacheFreshness::Invalid);
    }

    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_MODEL_CACHE_BYTES) as usize);
    if file
        .by_ref()
        .take(MAX_MODEL_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_MODEL_CACHE_BYTES
    {
        return Err(CodexModelCacheFreshness::Invalid);
    }
    Ok(bytes)
}

fn open_cache_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

