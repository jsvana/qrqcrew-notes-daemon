use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Organization {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub roster_url: String,
    /// Source type: "csv" (default) or "html_table"
    #[serde(default = "default_source_type")]
    pub source_type: String,
    /// Column name for callsigns (used for CSV sources)
    pub callsign_column: Option<String>,
    /// Column name for member ID (used for CSV sources)
    pub number_column: Option<String>,
    /// Column index for callsigns (used for HTML table sources)
    pub callsign_column_index: Option<usize>,
    /// Column index for member ID (used for HTML table sources)
    pub number_column_index: Option<usize>,
    #[serde(default)]
    pub skip_rows: usize,
    pub emoji: String,
    pub label: String,
    pub output_file: String,
    /// Optional per-organization GitHub settings (overrides global)
    pub github: Option<OrgGitHubConfig>,
}

fn default_source_type() -> String {
    "csv".to_string()
}

/// Per-organization GitHub config (all fields optional, falls back to global)
#[derive(Debug, Deserialize, Clone)]
pub struct OrgGitHubConfig {
    pub token: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub organizations: Vec<Organization>,
    pub github: GitHubConfig,
    pub daemon: DaemonConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub commit_author_name: String,
    pub commit_author_email: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DaemonConfig {
    pub sync_interval_secs: u64,
    pub run_once: bool,
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let config_path = path.unwrap_or_else(|| PathBuf::from("config.toml"));

        let builder = config::Config::builder()
            .add_source(config::File::from(config_path.clone()).required(true))
            .add_source(
                config::Environment::with_prefix("QRQCREW")
                    .separator("__")
                    .try_parsing(true),
            );

        let settings = builder
            .build()
            .with_context(|| format!("Failed to load config from {:?}", config_path))?;

        let mut config: Config = settings
            .try_deserialize()
            .context("Failed to deserialize config")?;

        // Handle ${VAR} placeholder in token fields
        if config.github.token.starts_with("${") && config.github.token.ends_with("}") {
            let env_var = &config.github.token[2..config.github.token.len() - 1];
            config.github.token = std::env::var(env_var)
                .with_context(|| format!("Environment variable {} not set", env_var))?;
        }

        // Handle ${VAR} placeholder in per-org token fields
        for org in &mut config.organizations {
            if let Some(ref mut gh) = org.github {
                if let Some(ref token) = gh.token {
                    if token.starts_with("${") && token.ends_with("}") {
                        let env_var = &token[2..token.len() - 1];
                        gh.token = Some(std::env::var(env_var).with_context(|| {
                            format!(
                                "Environment variable {} not set for org {}",
                                env_var, org.name
                            )
                        })?);
                    }
                }
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

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
        assert_eq!(
            config.organizations[0].callsign_column,
            Some("call".to_string())
        );
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

    #[test]
    fn test_config_per_org_github() {
        let config_content = r#"
[[organizations]]
name = "default_org"
roster_url = "https://example.com/default.csv"
callsign_column = "call"
number_column = "number"
emoji = "🔥"
label = "Default"
output_file = "default.txt"

[[organizations]]
name = "custom_org"
roster_url = "https://example.com/custom.csv"
callsign_column = "call"
number_column = "number"
emoji = "🎯"
label = "Custom"
output_file = "custom.txt"
[organizations.github]
owner = "custom_owner"
repo = "custom_repo"

[github]
token = "test_token"
owner = "global_owner"
repo = "global_repo"
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

        // First org should have no custom github config
        assert!(config.organizations[0].github.is_none());

        // Second org should have custom github config
        let org_github = config.organizations[1].github.as_ref().unwrap();
        assert_eq!(org_github.owner, Some("custom_owner".to_string()));
        assert_eq!(org_github.repo, Some("custom_repo".to_string()));
        assert!(org_github.branch.is_none()); // Not specified, should be None
    }
}
