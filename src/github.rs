use crate::config::GitHubConfig;
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use octocrab::Octocrab;
use tracing::{debug, info};

pub struct GitHubClient {
    client: Octocrab,
    owner: String,
    repo: String,
    branch: String,
    file_path: String,
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
            file_path: config.file_path.clone(),
            author_name: config.commit_author_name.clone(),
            author_email: config.commit_author_email.clone(),
        })
    }

    /// Get current file SHA (needed for updates)
    pub async fn get_file_sha(&self) -> Result<Option<String>> {
        let result = self
            .client
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(&self.file_path)
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
    pub async fn get_file_content(&self) -> Result<Option<String>> {
        let result = self
            .client
            .repos(&self.owner, &self.repo)
            .get_content()
            .path(&self.file_path)
            .r#ref(&self.branch)
            .send()
            .await;

        match result {
            Ok(content) => {
                if let Some(item) = content.items.first() {
                    if let Some(encoded_content) = &item.content {
                        // Content comes base64 encoded with newlines
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
    pub async fn commit_file(&self, content: &str, message: &str) -> Result<()> {
        let sha = self.get_file_sha().await?;

        debug!(
            "Committing to {}/{} branch {} file {}",
            self.owner, self.repo, self.branch, self.file_path
        );

        let repos = self.client.repos(&self.owner, &self.repo);
        let author = octocrab::models::repos::CommitAuthor {
            name: self.author_name.clone(),
            email: self.author_email.clone(),
            date: None,
        };

        match sha {
            Some(sha) => {
                // Update existing file
                repos
                    .update_file(&self.file_path, message, content, &sha)
                    .branch(&self.branch)
                    .commiter(author.clone())
                    .author(author)
                    .send()
                    .await
                    .context("Failed to update file")?;
            }
            None => {
                // Create new file
                repos
                    .create_file(&self.file_path, message, content)
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
            self.file_path, self.owner, self.repo
        );

        Ok(())
    }

    /// Check if content has changed (ignoring timestamp line)
    pub async fn content_changed(&self, new_content: &str) -> Result<bool> {
        match self.get_file_content().await? {
            Some(existing) => {
                // Compare ignoring timestamp line
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
            None => Ok(true), // File doesn't exist
        }
    }
}
