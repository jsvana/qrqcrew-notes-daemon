//! QRZ.com XML API client for callsign lookups
//!
//! Provides nickname (first name) lookups with session caching and auto-refresh.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

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
        if let Some(start) = xml.find("<Key>") {
            if let Some(end) = xml.find("</Key>") {
                let key = &xml[start + 5..end];
                return Ok(key.to_string());
            }
        }

        if xml.contains("<Error>") {
            if let Some(start) = xml.find("<Error>") {
                if let Some(end) = xml.find("</Error>") {
                    let error = &xml[start + 7..end];
                    anyhow::bail!("QRZ error: {}", error);
                }
            }
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

    /// Lookup callsign and return nickname (first name) if found
    pub async fn lookup_nickname(&self, callsign: &str) -> Result<Option<String>> {
        self.lookup_nickname_inner(callsign, 0).await
    }

    async fn lookup_nickname_inner(
        &self,
        callsign: &str,
        retry_count: u32,
    ) -> Result<Option<String>> {
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

        // Check for session timeout and retry
        if text.contains("Session Timeout") || text.contains("Invalid session key") {
            debug!("QRZ session expired, refreshing...");
            self.clear_session().await;
            return Box::pin(self.lookup_nickname_inner(callsign, retry_count + 1)).await;
        }

        // Check for "not found" error
        if text.contains("<Error>Not found") {
            debug!("Callsign {} not found in QRZ", callsign);
            return Ok(None);
        }

        Ok(Self::extract_fname(&text))
    }

    fn extract_fname(xml: &str) -> Option<String> {
        if let Some(start) = xml.find("<fname>") {
            if let Some(end) = xml[start..].find("</fname>") {
                let fname = &xml[start + 7..start + end];
                if !fname.is_empty() {
                    return Some(fname.to_string());
                }
            }
        }
        None
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

        let fname = QrzClient::extract_fname(xml);
        assert_eq!(fname, Some("Mike".to_string()));
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

        let fname = QrzClient::extract_fname(xml);
        assert_eq!(fname, None);
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

        let fname = QrzClient::extract_fname(xml);
        assert_eq!(fname, None);
    }
}
