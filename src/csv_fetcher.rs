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
    callsign_regex: Regex,
}

impl CsvFetcher {
    pub fn new(url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            url,
            // Callsign pattern: 1-2 letters, 1 digit, 1-4 letters
            callsign_regex: Regex::new(r"^[A-Z]{1,2}\d[A-Z]{1,4}$").unwrap(),
        }
    }

    pub async fn fetch_members(&self) -> Result<Vec<Member>> {
        let csv_data = self.fetch_with_retry(3).await?;

        // Create reader without headers first - we need to find the header row
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(csv_data.as_bytes());

        // Find the header row (first non-empty row with valid column names)
        let mut headers: Option<csv::StringRecord> = None;
        let mut header_row_num = 0;
        let mut records_iter = reader.records();

        for (row_num, result) in records_iter.by_ref().enumerate() {
            if let Ok(record) = result {
                // Check if this looks like a header row
                if let Some(first_col) = record.get(0) {
                    let first = first_col.to_lowercase().trim().to_string();
                    if first == "call" || first == "callsign" {
                        headers = Some(record);
                        header_row_num = row_num;
                        break;
                    }
                }
            }
        }

        let headers = headers.context("Could not find header row in CSV")?;
        debug!("Found header row at row {}", header_row_num + 1);

        let callsign_col = self
            .find_callsign_column(&headers)
            .context("Could not find callsign column in CSV")?;

        let qc_number_col = self
            .find_qc_number_column(&headers)
            .context("Could not find QC # column in CSV")?;

        debug!(
            "Using column {} for callsigns, column {} for QC #",
            callsign_col, qc_number_col
        );

        let mut seen: HashSet<String> = HashSet::new();
        let mut members: Vec<Member> = Vec::new();
        let data_start_row = header_row_num + 2; // 1-indexed, after header

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
                        let qc_number = match record.get(qc_number_col) {
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

    fn find_callsign_column(&self, headers: &csv::StringRecord) -> Option<usize> {
        // Look for common column names
        for (i, header) in headers.iter().enumerate() {
            let h = header.to_lowercase().trim().to_string();
            if h == "call" || h == "callsign" || h == "call sign" {
                return Some(i);
            }
        }

        // Fall back to first column if headers don't match
        // but verify first few rows look like callsigns
        if !headers.is_empty() {
            debug!("No callsign column found by name, defaulting to first column");
            return Some(0);
        }

        None
    }

    fn find_qc_number_column(&self, headers: &csv::StringRecord) -> Option<usize> {
        // Look for QC # column
        for (i, header) in headers.iter().enumerate() {
            let h = header.to_lowercase().trim().to_string();
            if h == "qc #" || h == "qc#" || h == "qc number" || h == "number" || h == "#" {
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

    #[test]
    fn test_callsign_validation() {
        let fetcher = CsvFetcher::new("http://example.com".to_string());

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
    fn test_find_callsign_column() {
        let fetcher = CsvFetcher::new("http://example.com".to_string());

        let headers = csv::StringRecord::from(vec!["Name", "Call", "Number"]);
        assert_eq!(fetcher.find_callsign_column(&headers), Some(1));

        let headers = csv::StringRecord::from(vec!["Callsign", "Name", "QC#"]);
        assert_eq!(fetcher.find_callsign_column(&headers), Some(0));

        let headers = csv::StringRecord::from(vec!["Member", "Call Sign", "Date"]);
        assert_eq!(fetcher.find_callsign_column(&headers), Some(1));

        // No matching header, defaults to first column
        let headers = csv::StringRecord::from(vec!["K4MW", "John", "1"]);
        assert_eq!(fetcher.find_callsign_column(&headers), Some(0));
    }

    #[test]
    fn test_find_qc_number_column() {
        let fetcher = CsvFetcher::new("http://example.com".to_string());

        let headers = csv::StringRecord::from(vec!["Callsign", "Name", "QC #"]);
        assert_eq!(fetcher.find_qc_number_column(&headers), Some(2));

        let headers = csv::StringRecord::from(vec!["Call", "QC#", "Date"]);
        assert_eq!(fetcher.find_qc_number_column(&headers), Some(1));

        let headers = csv::StringRecord::from(vec!["Member", "#", "Notes"]);
        assert_eq!(fetcher.find_qc_number_column(&headers), Some(1));

        // No matching header
        let headers = csv::StringRecord::from(vec!["Callsign", "Name", "Date"]);
        assert_eq!(fetcher.find_qc_number_column(&headers), None);
    }
}
