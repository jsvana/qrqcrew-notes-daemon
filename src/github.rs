use crate::config::{GitHubConfig, OrgGitHubConfig};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use octocrab::Octocrab;
use tracing::{debug, info};

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
        Self::with_overrides(config, None)
    }

    /// Create a GitHubClient with optional per-org overrides for owner/repo/branch
    pub fn with_overrides(
        config: &GitHubConfig,
        org_config: Option<&OrgGitHubConfig>,
    ) -> Result<Self> {
        let client = Octocrab::builder()
            .personal_token(config.token.clone())
            .build()
            .context("Failed to build GitHub client")?;

        let (owner, repo, branch) = match org_config {
            Some(org) => (
                org.owner.clone().unwrap_or_else(|| config.owner.clone()),
                org.repo.clone().unwrap_or_else(|| config.repo.clone()),
                org.branch.clone().unwrap_or_else(|| config.branch.clone()),
            ),
            None => (
                config.owner.clone(),
                config.repo.clone(),
                config.branch.clone(),
            ),
        };

        Ok(Self {
            client,
            owner,
            repo,
            branch,
            author_name: config.commit_author_name.clone(),
            author_email: config.commit_author_email.clone(),
        })
    }

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

        info!("Committed {} to {}/{}", file_path, self.owner, self.repo);

        Ok(())
    }

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
