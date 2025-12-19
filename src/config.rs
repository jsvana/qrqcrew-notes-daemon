use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub roster_url: String,
    pub github: GitHubConfig,
    pub daemon: DaemonConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitHubConfig {
    pub token: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub file_path: String,
    pub commit_author_name: String,
    pub commit_author_email: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DaemonConfig {
    pub sync_interval_secs: u64,
    pub run_once: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OutputConfig {
    pub emoji: String,
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

        // Handle ${GITHUB_TOKEN} placeholder in token field
        if config.github.token.starts_with("${") && config.github.token.ends_with("}") {
            let env_var = &config.github.token[2..config.github.token.len() - 1];
            config.github.token = std::env::var(env_var)
                .with_context(|| format!("Environment variable {} not set", env_var))?;
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
roster_url = "https://example.com/roster.csv"

[github]
token = "test_token"
owner = "testowner"
repo = "testrepo"
branch = "main"
file_path = "test.txt"
commit_author_name = "Test Bot"
commit_author_email = "test@example.com"

[daemon]
sync_interval_secs = 3600
run_once = true

[output]
emoji = "⚓"
"#;

        // Use .toml suffix so config crate recognizes the format
        let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
        temp_file.write_all(config_content.as_bytes()).unwrap();

        let config = Config::load(Some(temp_file.path().to_path_buf())).unwrap();

        assert_eq!(config.roster_url, "https://example.com/roster.csv");
        assert_eq!(config.github.owner, "testowner");
        assert_eq!(config.daemon.sync_interval_secs, 3600);
        assert!(config.daemon.run_once);
        assert_eq!(config.output.emoji, "⚓");
    }
}
