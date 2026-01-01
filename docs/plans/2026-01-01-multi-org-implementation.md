# Multi-Organization Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add support for multiple amateur radio organizations (QRQ Crew, CWops) with separate output files per org.

**Architecture:** Config-driven organization definitions with explicit CSV column mappings. Each org syncs independently with its own output file. GitHubClient modified to accept file path per-call rather than at construction.

**Tech Stack:** Rust, serde for config, reqwest for HTTP, octocrab for GitHub API

---

### Task 1: Add Organization struct to config

**Files:**
- Modify: `src/config.rs:5-33`

**Step 1: Add Organization struct after imports**

Add after line 3:

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct Organization {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub roster_url: String,
    pub callsign_column: String,
    pub number_column: String,
    #[serde(default)]
    pub skip_rows: usize,
    pub emoji: String,
    pub label: String,
    pub output_file: String,
}

fn default_enabled() -> bool {
    true
}
```

**Step 2: Update Config struct**

Replace lines 5-11:

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub organizations: Vec<Organization>,
    pub github: GitHubConfig,
    pub daemon: DaemonConfig,
}
```

**Step 3: Remove OutputConfig struct**

Delete lines 30-33 (`OutputConfig` struct) - no longer needed.

**Step 4: Remove file_path from GitHubConfig**

Replace lines 13-22:

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub commit_author_name: String,
    pub commit_author_email: String,
}
```

**Step 5: Build to verify syntax**

Run: `cargo build 2>&1 | head -50`
Expected: Errors about missing fields (tests will fail, that's OK for now)

**Step 6: Commit config struct changes**

```bash
git add src/config.rs
git commit -m "refactor(config): add Organization struct for multi-org support"
```

---

### Task 2: Update config tests

**Files:**
- Modify: `src/config.rs:66-106`

**Step 1: Replace test_config_load test**

Replace the entire test function (lines 72-105):

```rust
    #[test]
    fn test_config_load() {
        let config_content = r#"
[[organizations]]
name = "qrqcrew"
enabled = true
roster_url = "https://example.com/roster.csv"
callsign_column = "call"
number_column = "qc #"
skip_rows = 0
emoji = "⚓"
label = "QRQ Crew"
output_file = "qrqcrew-notes.txt"

[[organizations]]
name = "cwops"
enabled = false
roster_url = "https://example.com/cwops.csv"
callsign_column = "Callsign"
number_column = "Number"
skip_rows = 6
emoji = "🎹"
label = "CWops"
output_file = "cwops-notes.txt"

[github]
token = "test_token"
owner = "testowner"
repo = "testrepo"
branch = "main"
commit_author_name = "Test Bot"
commit_author_email = "test@example.com"

[daemon]
sync_interval_secs = 3600
run_once = true
"#;

        let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();

        let config = Config::load(Some(temp_file.path().to_path_buf())).unwrap();

        assert_eq!(config.organizations.len(), 2);
        assert_eq!(config.organizations[0].name, "qrqcrew");
        assert!(config.organizations[0].enabled);
        assert_eq!(config.organizations[0].callsign_column, "call");
        assert_eq!(config.organizations[1].name, "cwops");
        assert!(!config.organizations[1].enabled);
        assert_eq!(config.organizations[1].skip_rows, 6);
        assert_eq!(config.github.owner, "testowner");
        assert_eq!(config.daemon.sync_interval_secs, 3600);
    }

    #[test]
    fn test_config_default_enabled() {
        let config_content = r#"
[[organizations]]
name = "test"
roster_url = "https://example.com/test.csv"
callsign_column = "call"
number_column = "number"
emoji = "🔥"
label = "Test"
output_file = "test.txt"

[github]
token = "test_token"
owner = "testowner"
repo = "testrepo"
branch = "main"
commit_author_name = "Test Bot"
commit_author_email = "test@example.com"

[daemon]
sync_interval_secs = 3600
run_once = true
"#;

        let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();

        let config = Config::load(Some(temp_file.path().to_path_buf())).unwrap();

        // enabled should default to true when not specified
        assert!(config.organizations[0].enabled);
        // skip_rows should default to 0
        assert_eq!(config.organizations[0].skip_rows, 0);
    }
```

**Step 2: Run config tests**

Run: `cargo test config --lib`
Expected: PASS

**Step 3: Commit**

```bash
git add src/config.rs
git commit -m "test(config): update tests for multi-org config format"
```

---

### Task 3: Update CsvFetcher for explicit columns

**Files:**
- Modify: `src/csv_fetcher.rs:13-32`

**Step 1: Update CsvFetcher struct and new()**

Replace lines 13-32:

```rust
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
```

**Step 2: Update fetch_members to use skip_rows and explicit columns**

Replace lines 34-76 (the header finding and column detection logic):

```rust
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
```

**Step 3: Add find_column_by_name method, remove old methods**

Replace lines 173-202 (the old find_callsign_column and find_qc_number_column):

```rust
    fn find_column_by_name(&self, headers: &csv::StringRecord, name: &str) -> Option<usize> {
        let target = name.to_lowercase();
        for (i, header) in headers.iter().enumerate() {
            if header.to_lowercase().trim() == target {
                return Some(i);
            }
        }
        None
    }
```

**Step 4: Build to check syntax**

Run: `cargo build 2>&1 | head -30`
Expected: Errors about CsvFetcher::new() calls (main.rs) - that's expected

**Step 5: Commit**

```bash
git add src/csv_fetcher.rs
git commit -m "refactor(csv_fetcher): use explicit column names and skip_rows"
```

---

### Task 4: Update CsvFetcher tests

**Files:**
- Modify: `src/csv_fetcher.rs:209-268`

**Step 1: Replace test functions**

Replace lines 209-268:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_callsign_validation() {
        let fetcher = CsvFetcher::new(
            "http://example.com".to_string(),
            "call".to_string(),
            "number".to_string(),
            0,
        );

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
        assert!(!fetcher.is_valid_callsign("W6JSVX1"));
    }

    #[test]
    fn test_find_column_by_name() {
        let fetcher = CsvFetcher::new(
            "http://example.com".to_string(),
            "Callsign".to_string(),
            "Number".to_string(),
            0,
        );

        let headers = csv::StringRecord::from(vec!["Name", "Callsign", "Number"]);
        assert_eq!(fetcher.find_column_by_name(&headers, "Callsign"), Some(1));
        assert_eq!(fetcher.find_column_by_name(&headers, "callsign"), Some(1)); // case insensitive
        assert_eq!(fetcher.find_column_by_name(&headers, "Number"), Some(2));
        assert_eq!(fetcher.find_column_by_name(&headers, "Missing"), None);
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
```

**Step 2: Run tests**

Run: `cargo test csv_fetcher --lib`
Expected: PASS

**Step 3: Commit**

```bash
git add src/csv_fetcher.rs
git commit -m "test(csv_fetcher): update tests for explicit column config"
```

---

### Task 5: Update NotesGenerator for org config

**Files:**
- Modify: `src/notes_generator.rs:1-39`

**Step 1: Update NotesGenerator struct and methods**

Replace lines 1-39:

```rust
use crate::csv_fetcher::Member;
use chrono::Utc;

pub struct NotesGenerator {
    emoji: String,
    label: String,
    url: String,
}

impl NotesGenerator {
    pub fn new(emoji: String, label: String, url: Option<String>) -> Self {
        Self {
            emoji,
            label,
            url: url.unwrap_or_default(),
        }
    }

    pub fn generate(&self, members: &[Member]) -> String {
        let mut output = String::new();

        // Header comments
        output.push_str(&format!("# {} Callsign Notes for Ham2K PoLo\n", self.label));
        output.push_str(&format!(
            "# Generated: {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        if !self.url.is_empty() {
            output.push_str(&format!("# {}\n", self.url));
        }
        output.push_str("# Do not edit manually - this file is auto-generated\n");
        output.push('\n');

        // Sort and generate entries
        let mut sorted: Vec<_> = members.iter().collect();
        sorted.sort_by(|a, b| a.callsign.cmp(&b.callsign));

        for member in sorted {
            output.push_str(&format!(
                "{} {} {} #{}\n",
                member.callsign, self.emoji, self.label, member.qc_number
            ));
        }

        output
    }
}
```

**Step 2: Build to check**

Run: `cargo build 2>&1 | head -20`
Expected: Errors about NotesGenerator::new() calls - expected

**Step 3: Commit**

```bash
git add src/notes_generator.rs
git commit -m "refactor(notes_generator): accept label and url for org-specific output"
```

---

### Task 6: Update NotesGenerator tests

**Files:**
- Modify: `src/notes_generator.rs:41-98`

**Step 1: Replace test functions**

Replace lines 41-98:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_notes() {
        let generator = NotesGenerator::new(
            "⚓".to_string(),
            "QRQ Crew".to_string(),
            Some("https://qrqcrew.club".to_string()),
        );

        let members = vec![
            Member {
                callsign: "W6JSV".to_string(),
                qc_number: 10,
            },
            Member {
                callsign: "K4MW".to_string(),
                qc_number: 1,
            },
            Member {
                callsign: "WN7JT".to_string(),
                qc_number: 2,
            },
        ];

        let output = generator.generate(&members);

        // Check header
        assert!(output.contains("# QRQ Crew Callsign Notes for Ham2K PoLo"));
        assert!(output.contains("# Generated:"));
        assert!(output.contains("# https://qrqcrew.club"));

        // Check entries are sorted by callsign
        let lines: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "K4MW ⚓ QRQ Crew #1");
        assert_eq!(lines[1], "W6JSV ⚓ QRQ Crew #10");
        assert_eq!(lines[2], "WN7JT ⚓ QRQ Crew #2");
    }

    #[test]
    fn test_generate_cwops_format() {
        let generator = NotesGenerator::new(
            "🎹".to_string(),
            "CWops".to_string(),
            Some("https://cwops.org".to_string()),
        );

        let members = vec![Member {
            callsign: "W6JSV".to_string(),
            qc_number: 1234,
        }];

        let output = generator.generate(&members);

        assert!(output.contains("# CWops Callsign Notes for Ham2K PoLo"));
        assert!(output.contains("W6JSV 🎹 CWops #1234"));
    }

    #[test]
    fn test_generate_empty() {
        let generator = NotesGenerator::new("⚓".to_string(), "Test".to_string(), None);
        let output = generator.generate(&[]);

        assert!(output.contains("# Test Callsign Notes"));
        assert!(!output.contains("# https://")); // No URL when None

        let entries: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert!(entries.is_empty());
    }
}
```

**Step 2: Run tests**

Run: `cargo test notes_generator --lib`
Expected: PASS

**Step 3: Commit**

```bash
git add src/notes_generator.rs
git commit -m "test(notes_generator): update tests for org-specific labels"
```

---

### Task 7: Update GitHubClient to accept file path per call

**Files:**
- Modify: `src/github.rs:7-32`

**Step 1: Remove file_path from struct**

Replace lines 7-32:

```rust
pub struct GitHubClient {
    client: Octocrab,
    owner: String,
    repo: String,
    branch: String,
    author_name: String,
    author_email: String,
}

impl GitHubClient {
    pub fn new(config: &GitHubConfig) -> Result<Self> {
        let client = Octocrab::builder()
            .personal_token(config.token.clone())
            .build()
            .context("Failed to build GitHub client")?;

        Ok(Self {
            client,
            owner: config.owner.clone(),
            repo: config.repo.clone(),
            branch: config.branch.clone(),
            author_name: config.commit_author_name.clone(),
            author_email: config.commit_author_email.clone(),
        })
    }
```

**Step 2: Update get_file_sha to take file_path**

Replace lines 35-59:

```rust
    /// Get current file SHA (needed for updates)
    pub async fn get_file_sha(&self, file_path: &str) -> Result<Option<String>> {
        let result = self
            .client
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(file_path)
            .r#ref(&self.branch)
            .send()
            .await;

        match result {
            Ok(content) => {
                if let Some(item) = content.items.first() {
                    Ok(item.sha.clone().into())
                } else {
                    Ok(None)
                }
            }
            Err(octocrab::Error::GitHub { source, .. }) if source.message.contains("Not Found") => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
```

**Step 3: Update get_file_content to take file_path**

Replace lines 61-99:

```rust
    /// Get current file content
    pub async fn get_file_content(&self, file_path: &str) -> Result<Option<String>> {
        let result = self
            .client
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(file_path)
            .r#ref(&self.branch)
            .send()
            .await;

        match result {
            Ok(content) => {
                if let Some(item) = content.items.first() {
                    if let Some(encoded_content) = &item.content {
                        let clean_content: String = encoded_content
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        let decoded = STANDARD
                            .decode(&clean_content)
                            .context("Failed to decode base64 content")?;
                        let text = String::from_utf8(decoded)
                            .context("File content is not valid UTF-8")?;
                        Ok(Some(text))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            Err(octocrab::Error::GitHub { source, .. }) if source.message.contains("Not Found") => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
```

**Step 4: Update commit_file to take file_path**

Replace lines 101-148:

```rust
    /// Commit file (create or update)
    pub async fn commit_file(&self, file_path: &str, content: &str, message: &str) -> Result<()> {
        let sha = self.get_file_sha(file_path).await?;

        debug!(
            "Committing to {}/{} branch {} file {}",
            self.owner, self.repo, self.branch, file_path
        );

        let repos = self.client.repos(&self.owner, &self.repo);
        let author = octocrab::models::repos::CommitAuthor {
            name: self.author_name.clone(),
            email: self.author_email.clone(),
            date: None,
        };

        match sha {
            Some(sha) => {
                repos
                    .update_file(file_path, message, content, &sha)
                    .branch(&self.branch)
                    .commiter(author.clone())
                    .author(author)
                    .send()
                    .await
                    .context("Failed to update file")?;
            }
            None => {
                repos
                    .create_file(file_path, message, content)
                    .branch(&self.branch)
                    .commiter(author.clone())
                    .author(author)
                    .send()
                    .await
                    .context("Failed to create file")?;
            }
        }

        info!(
            "Committed {} to {}/{}",
            file_path, self.owner, self.repo
        );

        Ok(())
    }
```

**Step 5: Update content_changed to take file_path**

Replace lines 150-167:

```rust
    /// Check if content has changed (ignoring timestamp line)
    pub async fn content_changed(&self, file_path: &str, new_content: &str) -> Result<bool> {
        match self.get_file_content(file_path).await? {
            Some(existing) => {
                let existing_lines: Vec<&str> = existing
                    .lines()
                    .filter(|l| !l.starts_with("# Generated:"))
                    .collect();
                let new_lines: Vec<&str> = new_content
                    .lines()
                    .filter(|l| !l.starts_with("# Generated:"))
                    .collect();
                Ok(existing_lines != new_lines)
            }
            None => Ok(true),
        }
    }
}
```

**Step 6: Build to check**

Run: `cargo build 2>&1 | head -20`
Expected: Errors in main.rs about method signatures - expected

**Step 7: Commit**

```bash
git add src/github.rs
git commit -m "refactor(github): accept file_path as parameter instead of config"
```

---

### Task 8: Update main.rs for multi-org loop

**Files:**
- Modify: `src/main.rs`

**Step 1: Replace entire main.rs**

```rust
use anyhow::Result;
use clap::Parser;
use qrqcrew_notes_daemon::{Config, CsvFetcher, GitHubClient, NotesGenerator};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "qrqcrew-notes-daemon")]
#[command(about = "Generate Ham2K PoLo callsign notes from amateur radio organization rosters")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// Run once and exit (override config)
    #[arg(long)]
    once: bool,

    /// Dry run - don't commit to GitHub
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("qrqcrew_notes_daemon=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::load(Some(cli.config))?;

    let run_once = cli.once || config.daemon.run_once;

    let enabled_orgs: Vec<_> = config
        .organizations
        .iter()
        .filter(|o| o.enabled)
        .collect();

    if enabled_orgs.is_empty() {
        warn!("No organizations enabled in config");
        return Ok(());
    }

    info!(
        "Starting callsign notes daemon with {} enabled organization(s)",
        enabled_orgs.len()
    );

    let github = GitHubClient::new(&config.github)?;

    loop {
        for org in &enabled_orgs {
            info!("[{}] Starting sync", org.name);
            if let Err(e) = sync_org(org, &github, cli.dry_run).await {
                error!("[{}] Sync failed: {}", org.name, e);
            }
        }

        if run_once {
            info!("Run-once mode, exiting");
            break;
        }

        info!("Sleeping for {} seconds", config.daemon.sync_interval_secs);
        sleep(Duration::from_secs(config.daemon.sync_interval_secs)).await;
    }

    Ok(())
}

async fn sync_org(
    org: &qrqcrew_notes_daemon::config::Organization,
    github: &GitHubClient,
    dry_run: bool,
) -> Result<()> {
    // 1. Fetch roster
    let fetcher = CsvFetcher::new(
        org.roster_url.clone(),
        org.callsign_column.clone(),
        org.number_column.clone(),
        org.skip_rows,
    );
    let members = fetcher.fetch_members().await?;
    info!("[{}] Fetched {} members from roster", org.name, members.len());

    if members.is_empty() {
        warn!("[{}] No members found in roster, skipping", org.name);
        return Ok(());
    }

    // 2. Generate notes file
    let generator = NotesGenerator::new(org.emoji.clone(), org.label.clone(), None);
    let content = generator.generate(&members);

    if dry_run {
        info!("[{}] Dry run - would generate:\n{}", org.name, content);
        return Ok(());
    }

    // 3. Check if changed and commit
    if !github.content_changed(&org.output_file, &content).await? {
        info!("[{}] No changes detected, skipping commit", org.name);
        return Ok(());
    }

    let commit_msg = format!(
        "Update {} callsign notes ({} members)\n\nGenerated by qrqcrew-notes-daemon",
        org.label, members.len()
    );

    github.commit_file(&org.output_file, &content, &commit_msg).await?;
    info!("[{}] Successfully committed to {}", org.name, org.output_file);

    Ok(())
}
```

**Step 2: Export Organization from lib.rs**

Modify `src/lib.rs` to export Organization:

```rust
pub mod config;
pub mod csv_fetcher;
pub mod github;
pub mod notes_generator;

pub use config::{Config, Organization};
pub use csv_fetcher::{CsvFetcher, Member};
pub use github::GitHubClient;
pub use notes_generator::NotesGenerator;
```

**Step 3: Build**

Run: `cargo build`
Expected: PASS

**Step 4: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/main.rs src/lib.rs
git commit -m "feat(main): implement multi-org sync loop"
```

---

### Task 9: Update example config

**Files:**
- Modify: `config/config.example.toml`

**Step 1: Replace example config**

```toml
# QRQ Crew organization
[[organizations]]
name = "qrqcrew"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/e/2PACX-1vRBfNWrtgvUxTJQL96aK4g7ctZZ-Z572mBEbsscarGQWrbHg66yfxf-Jxw-bZ1ke7KX0zhJk6nUFWhL/pub?output=csv"
callsign_column = "call"
number_column = "qc #"
skip_rows = 0
emoji = "⚓"
label = "QRQ Crew"
output_file = "qrqcrew-notes.txt"

# CWops organization
[[organizations]]
name = "cwops"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/1Ew8b1WAorFRCixGRsr031atxmS0SsycvmOczS_fDqzc/export?format=csv"
callsign_column = "Callsign"
number_column = "Number"
skip_rows = 6
emoji = "🎹"
label = "CWops"
output_file = "cwops-notes.txt"

[github]
token = "${GITHUB_TOKEN}"  # Use environment variable
owner = "jsvana"
repo = "qrqcrew-site"
branch = "main"
commit_author_name = "QRQ Crew Bot"
commit_author_email = "qrqcrew-bot@noreply.github.com"

[daemon]
sync_interval_secs = 3600  # 1 hour
run_once = false
```

**Step 2: Commit**

```bash
git add config/config.example.toml
git commit -m "docs(config): update example config for multi-org format"
```

---

### Task 10: End-to-end dry-run test

**Step 1: Create test config**

Create a temporary config file for testing with both orgs:

```bash
cat > /tmp/test-config.toml << 'EOF'
[[organizations]]
name = "qrqcrew"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/e/2PACX-1vRBfNWrtgvUxTJQL96aK4g7ctZZ-Z572mBEbsscarGQWrbHg66yfxf-Jxw-bZ1ke7KX0zhJk6nUFWhL/pub?output=csv"
callsign_column = "call"
number_column = "qc #"
skip_rows = 0
emoji = "⚓"
label = "QRQ Crew"
output_file = "qrqcrew-notes.txt"

[[organizations]]
name = "cwops"
enabled = true
roster_url = "https://docs.google.com/spreadsheets/d/1Ew8b1WAorFRCixGRsr031atxmS0SsycvmOczS_fDqzc/export?format=csv"
callsign_column = "Callsign"
number_column = "Number"
skip_rows = 6
emoji = "🎹"
label = "CWops"
output_file = "cwops-notes.txt"

[github]
token = "fake"
owner = "test"
repo = "test"
branch = "main"
commit_author_name = "Test"
commit_author_email = "test@test.com"

[daemon]
sync_interval_secs = 3600
run_once = true
EOF
```

**Step 2: Run dry-run**

Run: `cargo run -- --config /tmp/test-config.toml --once --dry-run 2>&1`

Expected output should show:
- `[qrqcrew] Fetched N members from roster`
- QRQ Crew formatted output with `⚓ QRQ Crew #N`
- `[cwops] Fetched M members from roster`
- CWops formatted output with `🎹 CWops #N`

**Step 3: Verify output format**

Check that both orgs produce correctly formatted output.

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat: add multi-organization support

- Config now supports multiple organizations via [[organizations]] array
- Each org has explicit column mapping (callsign_column, number_column)
- skip_rows handles metadata before CSV headers
- Separate output files per organization
- Independent error handling per org"
```

---

## Summary

After completing all tasks, the daemon will:
1. Load multiple organizations from config
2. Fetch each org's roster with proper column mapping
3. Generate org-specific output files
4. Commit each file independently to GitHub
5. Continue syncing even if one org fails
