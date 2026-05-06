//! QRZ.com XML API client for callsign lookups.
//!
//! Returns the operator's *current* callsign (from the `<call>` element of
//! the response) and their first name nickname (from `<fname>`). Querying an
//! old callsign returns the new one in `<call>`, so we use that as the
//! authoritative current callsign and remap roster entries accordingly.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

/// Result of a QRZ callsign lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrzInfo {
    /// The operator's current callsign per QRZ's `<call>` element. When the
    /// queried callsign has been replaced (e.g. via a vanity grant) this is
    /// the new callsign; otherwise it's the queried callsign.
    pub current_call: String,
    /// The operator's preferred first name from `<fname>`, if present.
    pub nickname: Option<String>,
}

/// QRZ API client with session caching
#[derive(Clone)]
pub struct QrzClient {
    username: String,
    password: String,
    http: reqwest::Client,
    session_key: Arc<RwLock<Option<String>>>,
}

impl QrzClient {
    pub fn new(username: String, password: String) -> Self {
        Self {
            username,
            password,
            http: reqwest::Client::new(),
            session_key: Arc::new(RwLock::new(None)),
        }
    }

    /// Login to QRZ and get session key
    async fn login(&self) -> Result<String> {
        let url = format!(
            "https://xmldata.qrz.com/xml/current/?username={}&password={}&agent=qrqcrew-notes-daemon",
            self.username, self.password
        );

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .context("QRZ login request failed")?;

        let text = response
            .text()
            .await
            .context("QRZ login response read failed")?;

        Self::extract_session_key(&text)
    }

    fn extract_session_key(xml: &str) -> Result<String> {
        if let Some(start) = xml.find("<Key>")
            && let Some(end) = xml.find("</Key>")
        {
            let key = &xml[start + 5..end];
            return Ok(key.to_string());
        }

        if xml.contains("<Error>")
            && let Some(start) = xml.find("<Error>")
            && let Some(end) = xml.find("</Error>")
        {
            let error = &xml[start + 7..end];
            anyhow::bail!("QRZ error: {}", error);
        }

        anyhow::bail!("Could not parse QRZ session key")
    }

    async fn get_session_key(&self) -> Result<String> {
        {
            let cached = self.session_key.read().await;
            if let Some(ref key) = *cached {
                return Ok(key.clone());
            }
        }

        let key = self.login().await?;

        {
            let mut cached = self.session_key.write().await;
            *cached = Some(key.clone());
        }

        Ok(key)
    }

    async fn clear_session(&self) {
        let mut cached = self.session_key.write().await;
        *cached = None;
    }

    /// Look up a callsign and return `{current_call, nickname}`.
    ///
    /// Returns `None` when QRZ reports the callsign is not found. On every
    /// other error (network, session expiry after one retry, malformed XML)
    /// returns `Err`.
    pub async fn lookup(&self, callsign: &str) -> Result<Option<QrzInfo>> {
        self.lookup_inner(callsign, 0).await
    }

    async fn lookup_inner(&self, callsign: &str, retry_count: u32) -> Result<Option<QrzInfo>> {
        if retry_count > 1 {
            anyhow::bail!("QRZ lookup failed after retries");
        }

        let session_key = self.get_session_key().await?;

        let url = format!(
            "https://xmldata.qrz.com/xml/current/?s={}&callsign={}",
            session_key, callsign
        );

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .context("QRZ lookup request failed")?;

        let text = response
            .text()
            .await
            .context("QRZ lookup response read failed")?;

        // Session expired — clear cache and retry once.
        if text.contains("Session Timeout") || text.contains("Invalid session key") {
            debug!("QRZ session expired, refreshing...");
            self.clear_session().await;
            return Box::pin(self.lookup_inner(callsign, retry_count + 1)).await;
        }

        if text.contains("<Error>Not found") {
            debug!("Callsign {} not found in QRZ", callsign);
            return Ok(None);
        }

        // QRZ returns the canonical `<call>` for the operator, which may
        // differ from the queried callsign for retired/aliased calls.
        let current_call = Self::extract_call(&text)
            .map(|c| c.to_uppercase())
            .unwrap_or_else(|| callsign.to_uppercase());
        let nickname = Self::extract_fname(&text);

        Ok(Some(QrzInfo {
            current_call,
            nickname,
        }))
    }

    fn extract_call(xml: &str) -> Option<String> {
        Self::extract_tag(xml, "call")
    }

    fn extract_fname(xml: &str) -> Option<String> {
        Self::extract_tag(xml, "fname")
    }

    /// Extract the first occurrence of `<tag>...</tag>` returning the inner
    /// text, or `None` if the tag is absent or empty.
    fn extract_tag(xml: &str, tag: &str) -> Option<String> {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        let start = xml.find(&open)?;
        let after_open = start + open.len();
        let end = xml[after_open..].find(&close)? + after_open;
        let value = &xml[after_open..end];
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_key() {
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Session>
    <Key>abc123sessionkey</Key>
    <Count>42</Count>
  </Session>
</QRZDatabase>"#;

        let key = QrzClient::extract_session_key(xml).unwrap();
        assert_eq!(key, "abc123sessionkey");
    }

    #[test]
    fn test_extract_session_key_error() {
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Session>
    <Error>Invalid username/password</Error>
  </Session>
</QRZDatabase>"#;

        let result = QrzClient::extract_session_key(xml);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid username/password")
        );
    }

    #[test]
    fn test_extract_fname() {
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Callsign>
    <call>K4MW</call>
    <fname>Mike</fname>
    <name>Smith</name>
  </Callsign>
</QRZDatabase>"#;

        assert_eq!(QrzClient::extract_fname(xml), Some("Mike".to_string()));
        assert_eq!(QrzClient::extract_call(xml), Some("K4MW".to_string()));
    }

    #[test]
    fn test_extract_fname_empty() {
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Callsign>
    <call>K4MW</call>
    <fname></fname>
  </Callsign>
</QRZDatabase>"#;

        assert_eq!(QrzClient::extract_fname(xml), None);
    }

    #[test]
    fn test_extract_fname_missing() {
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Callsign>
    <call>K4MW</call>
    <name>Smith</name>
  </Callsign>
</QRZDatabase>"#;

        assert_eq!(QrzClient::extract_fname(xml), None);
    }

    #[test]
    fn test_extract_call_returns_canonical_for_retired_call() {
        // Real shape returned by QRZ when querying a retired callsign:
        // <call> is the *new* canonical callsign; <xref> is the queried alias.
        let xml = r#"<?xml version="1.0" ?>
<QRZDatabase>
  <Callsign>
    <call>W6JY</call>
    <xref>W6JSV</xref>
    <aliases>W6JSV</aliases>
    <fname>James S</fname>
  </Callsign>
</QRZDatabase>"#;

        assert_eq!(QrzClient::extract_call(xml), Some("W6JY".to_string()));
    }
}
