//! Persistent file-based cache for QRZ lookups.
//!
//! Stores `queried-callsign -> {current_call, nickname}` mappings with TTL.
//! The cache is keyed on the *queried* callsign (i.e. what came out of the
//! roster), so retired/aliased callsigns continue to map cheaply to their
//! current canonical form on every refresh cycle.
//!
//! Legacy on-disk entries written before the `current_call` field existed
//! are accepted (the field defaults to `None`), but they're treated as
//! incomplete and force a re-lookup on `get()` — that way a single daemon
//! restart backfills `current_call` for the entire roster without throwing
//! away nicknames already stored.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::qrz::QrzInfo;

/// Default cache TTL: 30 days
const DEFAULT_TTL_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    /// Operator's first-name nickname from QRZ `<fname>`, if present.
    nickname: Option<String>,
    /// Operator's current canonical callsign from QRZ `<call>`. Optional for
    /// backwards compatibility with caches written before this field existed
    /// — those entries are treated as expired and re-looked up.
    #[serde(default)]
    current_call: Option<String>,
    cached_at: DateTime<Utc>,
}

impl CacheEntry {
    fn from_info(info: Option<&QrzInfo>) -> Self {
        Self {
            nickname: info.and_then(|i| i.nickname.clone()),
            current_call: info.map(|i| i.current_call.clone()),
            cached_at: Utc::now(),
        }
    }

    fn from_negative() -> Self {
        // Negative cache: callsign-not-found. We still record `current_call`
        // as a sentinel (the queried callsign itself, uppercased) so the
        // entry isn't mistaken for a legacy "needs backfill" row.
        Self {
            nickname: None,
            current_call: None,
            cached_at: Utc::now(),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        Utc::now() - self.cached_at > ttl
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheData {
    entries: HashMap<String, CacheEntry>,
}

/// Persistent QRZ-lookup cache with file-based storage.
pub struct NicknameCache {
    path: PathBuf,
    data: CacheData,
    ttl: Duration,
    dirty: bool,
}

/// What the cache returns for a previously-seen callsign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachedLookup {
    /// QRZ returned a record for this callsign.
    Found {
        current_call: String,
        nickname: Option<String>,
    },
    /// QRZ returned "not found" — we negatively cache to avoid re-querying.
    NotFound,
}

impl NicknameCache {
    /// Load cache from file, or create empty if not exists.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let ttl = Duration::days(DEFAULT_TTL_DAYS);

        let data = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read cache file: {}", path.display()))?;

            // Handle empty files
            if content.trim().is_empty() {
                debug!("Cache file is empty, starting fresh");
                CacheData::default()
            } else {
                let mut data: CacheData = serde_json::from_str(&content)
                    .with_context(|| format!("Failed to parse cache file: {}", path.display()))?;

                // Prune expired entries on load
                let before_count = data.entries.len();
                data.entries.retain(|_, entry| !entry.is_expired(ttl));
                let pruned = before_count - data.entries.len();

                if pruned > 0 {
                    info!(
                        "Loaded QRZ lookup cache with {} entries ({} expired, pruned)",
                        data.entries.len(),
                        pruned
                    );
                } else {
                    info!("Loaded QRZ lookup cache with {} entries", data.entries.len());
                }

                data
            }
        } else {
            debug!("No existing cache file, starting fresh");
            CacheData::default()
        };

        Ok(Self {
            path,
            data,
            ttl,
            dirty: false,
        })
    }

    /// Look up a previously-cached callsign.
    ///
    /// Returns `None` when the entry is missing, expired, or is a legacy
    /// row written before `current_call` existed (forcing a re-lookup so
    /// the new field gets populated).
    pub fn get(&self, callsign: &str) -> Option<CachedLookup> {
        let callsign = callsign.to_uppercase();
        let entry = self.data.entries.get(&callsign)?;
        if entry.is_expired(self.ttl) {
            return None;
        }
        match (&entry.current_call, &entry.nickname) {
            (Some(cc), nick) => Some(CachedLookup::Found {
                current_call: cc.clone(),
                nickname: nick.clone(),
            }),
            (None, None) => Some(CachedLookup::NotFound),
            // Legacy row: nickname populated, current_call missing. Force a
            // re-lookup so we can backfill the canonical callsign.
            (None, Some(_)) => None,
        }
    }

    /// Insert a positive QRZ result.
    pub fn insert_found(&mut self, queried: &str, info: &QrzInfo) {
        let queried = queried.to_uppercase();
        self.data
            .entries
            .insert(queried, CacheEntry::from_info(Some(info)));
        self.dirty = true;
    }

    /// Insert a negative result (callsign not found).
    pub fn insert_not_found(&mut self, queried: &str) {
        let queried = queried.to_uppercase();
        self.data.entries.insert(queried, CacheEntry::from_negative());
        self.dirty = true;
    }

    /// Save cache to disk if dirty.
    pub fn save(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create cache directory: {}", parent.display())
            })?;
        }

        let content =
            serde_json::to_string_pretty(&self.data).context("Failed to serialize cache")?;

        std::fs::write(&self.path, content)
            .with_context(|| format!("Failed to write cache file: {}", self.path.display()))?;

        self.dirty = false;
        debug!("Saved QRZ lookup cache ({} entries)", self.data.entries.len());

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.data.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.entries.is_empty()
    }
}

impl Drop for NicknameCache {
    fn drop(&mut self) {
        if self.dirty
            && let Err(e) = self.save()
        {
            warn!("Failed to save cache on drop: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn info(current: &str, nick: Option<&str>) -> QrzInfo {
        QrzInfo {
            current_call: current.to_string(),
            nickname: nick.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_cache_insert_and_get_found() {
        let temp = NamedTempFile::new().unwrap();
        let mut cache = NicknameCache::load(temp.path()).unwrap();

        cache.insert_found("W6JSV", &info("W6JY", Some("Jay")));
        cache.insert_found("K4MW", &info("K4MW", None));

        assert_eq!(
            cache.get("W6JSV"),
            Some(CachedLookup::Found {
                current_call: "W6JY".to_string(),
                nickname: Some("Jay".to_string())
            })
        );
        // Case insensitive
        assert_eq!(
            cache.get("w6jsv"),
            Some(CachedLookup::Found {
                current_call: "W6JY".to_string(),
                nickname: Some("Jay".to_string())
            })
        );
        assert_eq!(
            cache.get("K4MW"),
            Some(CachedLookup::Found {
                current_call: "K4MW".to_string(),
                nickname: None
            })
        );
        assert_eq!(cache.get("N0CALL"), None);
    }

    #[test]
    fn test_cache_negative() {
        let temp = NamedTempFile::new().unwrap();
        let mut cache = NicknameCache::load(temp.path()).unwrap();

        cache.insert_not_found("ZZ9ZZZ");
        assert_eq!(cache.get("ZZ9ZZZ"), Some(CachedLookup::NotFound));
    }

    #[test]
    fn test_cache_persistence() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path().to_path_buf();

        {
            let mut cache = NicknameCache::load(&path).unwrap();
            cache.insert_found("W6JSV", &info("W6JY", Some("Jay")));
            cache.save().unwrap();
        }

        let cache = NicknameCache::load(&path).unwrap();
        assert_eq!(
            cache.get("W6JSV"),
            Some(CachedLookup::Found {
                current_call: "W6JY".to_string(),
                nickname: Some("Jay".to_string())
            })
        );
    }

    #[test]
    fn test_legacy_entry_forces_relookup() {
        // Hand-write a legacy cache file (no current_call field, nickname set).
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path().to_path_buf();
        let legacy = r#"{
            "entries": {
                "W6JSV": { "nickname": "Jay", "cached_at": "2026-04-01T00:00:00Z" }
            }
        }"#;
        std::fs::write(&path, legacy).unwrap();

        let cache = NicknameCache::load(&path).unwrap();
        // Returns None so the caller will hit QRZ and backfill current_call.
        assert_eq!(cache.get("W6JSV"), None);
    }
}
