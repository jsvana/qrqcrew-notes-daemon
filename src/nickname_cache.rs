//! Persistent file-based cache for QRZ nickname lookups
//!
//! Stores callsign -> nickname mappings with TTL to disk.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Default cache TTL: 30 days
const DEFAULT_TTL_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    nickname: Option<String>,
    cached_at: DateTime<Utc>,
}

impl CacheEntry {
    fn new(nickname: Option<String>) -> Self {
        Self {
            nickname,
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

/// Persistent nickname cache with file-based storage
pub struct NicknameCache {
    path: PathBuf,
    data: CacheData,
    ttl: Duration,
    dirty: bool,
}

impl NicknameCache {
    /// Load cache from file, or create empty if not exists
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
                        "Loaded nickname cache with {} entries ({} expired, pruned)",
                        data.entries.len(),
                        pruned
                    );
                } else {
                    info!("Loaded nickname cache with {} entries", data.entries.len());
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

    /// Get a cached nickname (returns None if not cached or expired)
    pub fn get(&self, callsign: &str) -> Option<Option<String>> {
        let callsign = callsign.to_uppercase();
        self.data.entries.get(&callsign).and_then(|entry| {
            if entry.is_expired(self.ttl) {
                None
            } else {
                Some(entry.nickname.clone())
            }
        })
    }

    /// Insert a nickname into the cache
    pub fn insert(&mut self, callsign: &str, nickname: Option<String>) {
        let callsign = callsign.to_uppercase();
        self.data
            .entries
            .insert(callsign, CacheEntry::new(nickname));
        self.dirty = true;
    }

    /// Get all callsigns that need lookup (not in cache or expired)
    pub fn filter_uncached<'a>(&self, callsigns: &'a [String]) -> Vec<&'a String> {
        callsigns
            .iter()
            .filter(|cs| self.get(cs).is_none())
            .collect()
    }

    /// Save cache to disk if dirty
    pub fn save(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }

        // Ensure parent directory exists
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
        debug!("Saved nickname cache ({} entries)", self.data.entries.len());

        Ok(())
    }

    /// Number of entries in the cache
    pub fn len(&self) -> usize {
        self.data.entries.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.data.entries.is_empty()
    }
}

impl Drop for NicknameCache {
    fn drop(&mut self) {
        if self.dirty
            && let Err(e) = self.save() {
                warn!("Failed to save cache on drop: {}", e);
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_cache_insert_and_get() {
        let temp = NamedTempFile::new().unwrap();
        let mut cache = NicknameCache::load(temp.path()).unwrap();

        cache.insert("W6JSV", Some("Jay".to_string()));
        cache.insert("K4MW", None);

        assert_eq!(cache.get("W6JSV"), Some(Some("Jay".to_string())));
        assert_eq!(cache.get("w6jsv"), Some(Some("Jay".to_string()))); // Case insensitive
        assert_eq!(cache.get("K4MW"), Some(None)); // Cached as "no nickname"
        assert_eq!(cache.get("N0CALL"), None); // Not cached
    }

    #[test]
    fn test_cache_persistence() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path().to_path_buf();

        // Write some entries
        {
            let mut cache = NicknameCache::load(&path).unwrap();
            cache.insert("W6JSV", Some("Jay".to_string()));
            cache.insert("K4MW", None);
            cache.save().unwrap();
        }

        // Load again and verify
        {
            let cache = NicknameCache::load(&path).unwrap();
            assert_eq!(cache.get("W6JSV"), Some(Some("Jay".to_string())));
            assert_eq!(cache.get("K4MW"), Some(None));
        }
    }

    #[test]
    fn test_filter_uncached() {
        let temp = NamedTempFile::new().unwrap();
        let mut cache = NicknameCache::load(temp.path()).unwrap();

        cache.insert("W6JSV", Some("Jay".to_string()));

        let callsigns = vec!["W6JSV".to_string(), "K4MW".to_string(), "WN7JT".to_string()];

        let uncached = cache.filter_uncached(&callsigns);
        assert_eq!(uncached.len(), 2);
        assert!(uncached.contains(&&"K4MW".to_string()));
        assert!(uncached.contains(&&"WN7JT".to_string()));
    }
}
