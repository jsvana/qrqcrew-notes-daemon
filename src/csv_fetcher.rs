use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct Member {
    pub callsign: String,
    pub qc_number: u32,
}

pub struct CsvFetcher {
    client: reqwest::Client,
    url: String,
    callsign_column: String,
    number_column: String,
    skip_rows: usize,
    callsign_regex: Regex,
}

impl CsvFetcher {
    pub fn new(url: String, callsign_column: String, number_column: String, skip_rows: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            url,
            callsign_column,
            number_column,
            skip_rows,
            callsign_regex: Regex::new(r"^[A-Z]{1,2}\d[A-Z]{1,4}$").unwrap(),
        }
    }

    pub async fn fetch_members(&self) -> Result<Vec<Member>> {
        let csv_data = self.fetch_with_retry(3).await?;

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(csv_data.as_bytes());

        let mut records_iter = reader.records();

        // Skip metadata rows
        for _ in 0..self.skip_rows {
            records_iter.next();
        }

        // Next row should be headers
        let headers = records_iter
            .next()
            .context("CSV has no header row after skipping metadata")?
            .context("Failed to parse header row")?;

        debug!("Header row: {:?}", headers);

        let callsign_col = self
            .find_column_by_name(&headers, &self.callsign_column)
            .with_context(|| format!("Could not find callsign column '{}' in CSV", self.callsign_column))?;

        let number_col = self
            .find_column_by_name(&headers, &self.number_column)
            .with_context(|| format!("Could not find number column '{}' in CSV", self.number_column))?;

        debug!(
            "Using column {} for callsigns, column {} for numbers",
            callsign_col, number_col
        );

        let mut seen: HashSet<String> = HashSet::new();
        let mut members: Vec<Member> = Vec::new();
        let data_start_row = self.skip_rows + 2; // 1-indexed, after header

        for (row_num, result) in records_iter.enumerate() {
            let actual_row = data_start_row + row_num;
            match result {
                Ok(record) => {
                    if let Some(callsign_raw) = record.get(callsign_col) {
                        let callsign = callsign_raw.trim().to_uppercase();

                        if callsign.is_empty() {
                            continue;
                        }

                        if !self.is_valid_callsign(&callsign) {
                            debug!("Row {}: Invalid callsign pattern: {}", actual_row, callsign);
                            continue;
                        }

                        if seen.contains(&callsign) {
                            debug!("Row {}: Duplicate callsign: {}", actual_row, callsign);
                            continue;
                        }

                        // Parse QC number
                        let qc_number = match record.get(number_col) {
                            Some(num_str) => match num_str.trim().parse::<u32>() {
                                Ok(n) => n,
                                Err(_) => {
                                    debug!(
                                        "Row {}: Invalid QC # '{}' for callsign {}",
                                        actual_row, num_str, callsign
                                    );
                                    continue;
                                }
                            },
                            None => {
                                debug!(
                                    "Row {}: Missing QC # for callsign {}",
                                    actual_row, callsign
                                );
                                continue;
                            }
                        };

                        seen.insert(callsign.clone());
                        members.push(Member {
                            callsign,
                            qc_number,
                        });
                    }
                }
                Err(e) => {
                    warn!("Row {}: Failed to parse: {}", actual_row, e);
                }
            }
        }

        // Sort alphabetically
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
                    last_error = Some(anyhow::anyhow!("Request failed: {}", e));
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

    fn find_column_by_name(&self, headers: &csv::StringRecord, name: &str) -> Option<usize> {
        let target = name.to_lowercase();
        for (i, header) in headers.iter().enumerate() {
            if header.to_lowercase().trim() == target {
                return Some(i);
            }
        }
        None
    }

    fn is_valid_callsign(&self, s: &str) -> bool {
        self.callsign_regex.is_match(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_fetcher() -> CsvFetcher {
        CsvFetcher::new(
            "http://example.com".to_string(),
            "Call".to_string(),
            "QC #".to_string(),
            0,
        )
    }

    #[test]
    fn test_callsign_validation() {
        let fetcher = test_fetcher();

        // Valid callsigns
        assert!(fetcher.is_valid_callsign("W6JSV"));
        assert!(fetcher.is_valid_callsign("K4MW"));
        assert!(fetcher.is_valid_callsign("WN7JT"));
        assert!(fetcher.is_valid_callsign("KI7QCF"));
        assert!(fetcher.is_valid_callsign("VK1AO"));
        assert!(fetcher.is_valid_callsign("N1A"));

        // Invalid callsigns
        assert!(!fetcher.is_valid_callsign(""));
        assert!(!fetcher.is_valid_callsign("INVALID"));
        assert!(!fetcher.is_valid_callsign("123"));
        assert!(!fetcher.is_valid_callsign("W6"));
        assert!(!fetcher.is_valid_callsign("W6JSVX1")); // Too many suffix letters
    }

    #[test]
    fn test_find_column_by_name() {
        let fetcher = test_fetcher();

        let headers = csv::StringRecord::from(vec!["Name", "Call", "Number"]);
        assert_eq!(fetcher.find_column_by_name(&headers, "Call"), Some(1));
        assert_eq!(fetcher.find_column_by_name(&headers, "call"), Some(1));
        assert_eq!(fetcher.find_column_by_name(&headers, "CALL"), Some(1));

        let headers = csv::StringRecord::from(vec!["Callsign", "Name", "QC #"]);
        assert_eq!(fetcher.find_column_by_name(&headers, "Callsign"), Some(0));
        assert_eq!(fetcher.find_column_by_name(&headers, "QC #"), Some(2));

        // Column not found
        let headers = csv::StringRecord::from(vec!["Callsign", "Name", "Date"]);
        assert_eq!(fetcher.find_column_by_name(&headers, "QC #"), None);
    }

    #[test]
    fn test_find_column_with_whitespace() {
        let fetcher = CsvFetcher::new(
            "http://example.com".to_string(),
            "call".to_string(),
            "qc #".to_string(),
            0,
        );

        let headers = csv::StringRecord::from(vec!["  call  ", "name", " qc # "]);
        assert_eq!(fetcher.find_column_by_name(&headers, "call"), Some(0));
        assert_eq!(fetcher.find_column_by_name(&headers, "qc #"), Some(2));
    }
}
