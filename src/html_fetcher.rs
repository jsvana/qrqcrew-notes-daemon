use anyhow::{Context, Result};
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;
use tracing::{debug, warn};

use crate::csv_fetcher::Member;

pub struct HtmlFetcher {
    client: reqwest::Client,
    url: String,
    callsign_column_index: usize,
    number_column_index: usize,
    callsign_regex: Regex,
}

impl HtmlFetcher {
    pub fn new(url: String, callsign_column_index: usize, number_column_index: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            url,
            callsign_column_index,
            number_column_index,
            callsign_regex: Regex::new(r"^[A-Z]{1,2}\d[A-Z]{1,4}$").unwrap(),
        }
    }

    pub async fn fetch_members(&self) -> Result<Vec<Member>> {
        let html = self.fetch_with_retry(3).await?;
        self.parse_html(&html)
    }

    fn parse_html(&self, html: &str) -> Result<Vec<Member>> {
        let document = Html::parse_document(html);

        // Select table rows - SKCC uses table.skcc_table
        let table_selector =
            Selector::parse("table.skcc_table tr").expect("Failed to parse table selector");
        let td_selector = Selector::parse("td").expect("Failed to parse td selector");

        let mut seen: HashSet<String> = HashSet::new();
        let mut members: Vec<Member> = Vec::new();

        for (row_num, row) in document.select(&table_selector).enumerate() {
            let cells: Vec<_> = row.select(&td_selector).collect();

            // Skip header rows (they use <th> not <td>)
            if cells.is_empty() {
                continue;
            }

            // Check we have enough columns
            if cells.len() <= self.callsign_column_index || cells.len() <= self.number_column_index
            {
                debug!("Row {}: Not enough columns ({})", row_num, cells.len());
                continue;
            }

            // Extract callsign
            let callsign_raw = cells[self.callsign_column_index]
                .text()
                .collect::<String>()
                .trim()
                .to_uppercase();

            // Skip Silent Keys (callsigns ending with /SK)
            if callsign_raw.ends_with("/SK") {
                debug!("Row {}: Skipping Silent Key: {}", row_num, callsign_raw);
                continue;
            }

            // Clean callsign (remove any suffix like /SK that might be partial)
            let callsign = callsign_raw
                .split('/')
                .next()
                .unwrap_or(&callsign_raw)
                .trim()
                .to_string();

            if callsign.is_empty() {
                continue;
            }

            if !self.is_valid_callsign(&callsign) {
                debug!("Row {}: Invalid callsign pattern: {}", row_num, callsign);
                continue;
            }

            if seen.contains(&callsign) {
                debug!("Row {}: Duplicate callsign: {}", row_num, callsign);
                continue;
            }

            // Extract member ID (SKCC number with possible suffix like 2C, 3S, etc.)
            let member_id = cells[self.number_column_index]
                .text()
                .collect::<String>()
                .trim()
                .to_string();

            if member_id.is_empty() {
                debug!("Row {}: Empty member ID for callsign {}", row_num, callsign);
                continue;
            }

            seen.insert(callsign.clone());
            members.push(Member {
                callsign,
                member_id,
                nickname: None,
            });
        }

        // Sort alphabetically by callsign
        members.sort_by(|a, b| a.callsign.cmp(&b.callsign));

        Ok(members)
    }

    async fn fetch_with_retry(&self, max_attempts: u32) -> Result<String> {
        let mut last_error = None;

        for attempt in 1..=max_attempts {
            match self.client.get(&self.url).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return response
                            .text()
                            .await
                            .context("Failed to read response body");
                    } else {
                        last_error = Some(anyhow::anyhow!("HTTP error: {}", status));
                    }
                }
                Err(e) => {
                    // Log the full error chain for debugging network issues
                    let mut error_chain = format!("Request failed: {}", e);
                    if let Some(source) = e.source() {
                        error_chain.push_str(&format!(" -> {}", source));
                    }
                    if e.is_connect() {
                        error_chain.push_str(" [connection error]");
                    }
                    if e.is_timeout() {
                        error_chain.push_str(" [timeout]");
                    }
                    last_error = Some(anyhow::anyhow!("{}", error_chain));
                }
            }

            if attempt < max_attempts {
                let delay = Duration::from_millis(500 * 2u64.pow(attempt - 1));
                warn!("Fetch attempt {} failed, retrying in {:?}", attempt, delay);
                tokio::time::sleep(delay).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown fetch error")))
    }

    fn is_valid_callsign(&self, s: &str) -> bool {
        self.callsign_regex.is_match(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skcc_html() {
        let html = r#"
        <table class="skcc_table">
            <tr>
                <th>SKCC #</th>
                <th>Call</th>
                <th>Name</th>
            </tr>
            <tr>
                <td>1</td>
                <td>KC9ECI</td>
                <td>Tom</td>
            </tr>
            <tr>
                <td>2C</td>
                <td>KI4CIA</td>
                <td>Melinda</td>
            </tr>
            <tr>
                <td>3S</td>
                <td>N6WK/SK</td>
                <td>Gordon [SK]</td>
            </tr>
        </table>
        "#;

        let fetcher = HtmlFetcher::new("http://example.com".to_string(), 1, 0);
        let members = fetcher.parse_html(html).unwrap();

        // Should have 2 members (Silent Key filtered out)
        assert_eq!(members.len(), 2);

        // Should be sorted alphabetically
        assert_eq!(members[0].callsign, "KC9ECI");
        assert_eq!(members[0].member_id, "1");

        assert_eq!(members[1].callsign, "KI4CIA");
        assert_eq!(members[1].member_id, "2C");
    }

    #[test]
    fn test_callsign_validation() {
        let fetcher = HtmlFetcher::new("http://example.com".to_string(), 1, 0);

        // Valid callsigns
        assert!(fetcher.is_valid_callsign("W6JSV"));
        assert!(fetcher.is_valid_callsign("K4MW"));
        assert!(fetcher.is_valid_callsign("KC9ECI"));
        assert!(fetcher.is_valid_callsign("VK1AO"));

        // Invalid callsigns
        assert!(!fetcher.is_valid_callsign(""));
        assert!(!fetcher.is_valid_callsign("INVALID"));
        assert!(!fetcher.is_valid_callsign("123"));
    }
}
