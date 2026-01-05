use crate::config::{GitHubConfig, OrgGitHubConfig};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use octocrab::Octocrab;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// A file pending commit in a batch operation
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub path: String,
    pub content: String,
    pub org_label: String,
    pub member_count: usize,
}

/// Git Data API request/response types
#[derive(Debug, Serialize)]
struct CreateBlobRequest {
    content: String,
    encoding: String,
}

#[derive(Debug, Deserialize)]
struct BlobResponse {
    sha: String,
}

#[derive(Debug, Serialize)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    entry_type: String,
    sha: String,
}

#[derive(Debug, Serialize)]
struct CreateTreeRequest {
    base_tree: String,
    tree: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    sha: String,
}

#[derive(Debug, Clone, Serialize)]
struct CommitAuthor {
    name: String,
    email: String,
}

#[derive(Debug, Serialize)]
struct CreateCommitRequest {
    message: String,
    tree: String,
    parents: Vec<String>,
    author: CommitAuthor,
    committer: CommitAuthor,
}

#[derive(Debug, Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Debug, Serialize)]
struct UpdateRefRequest {
    sha: String,
}

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

    /// Create a GitHubClient with optional per-org overrides for token/owner/repo/branch
    pub fn with_overrides(
        config: &GitHubConfig,
        org_config: Option<&OrgGitHubConfig>,
    ) -> Result<Self> {
        let (token, owner, repo, branch) = match org_config {
            Some(org) => (
                org.token.clone().unwrap_or_else(|| config.token.clone()),
                org.owner.clone().unwrap_or_else(|| config.owner.clone()),
                org.repo.clone().unwrap_or_else(|| config.repo.clone()),
                org.branch.clone().unwrap_or_else(|| config.branch.clone()),
            ),
            None => (
                config.token.clone(),
                config.owner.clone(),
                config.repo.clone(),
                config.branch.clone(),
            ),
        };

        info!(
            "Building GitHub client: owner={}, repo={}, branch={}, token_len={}",
            owner,
            repo,
            branch,
            token.len()
        );

        if token.is_empty() {
            anyhow::bail!("GitHub token is empty");
        }

        let client = Octocrab::builder()
            .personal_token(token)
            .build()
            .context("Failed to build GitHub client")?;

        info!("GitHub client built successfully");

        Ok(Self {
            client,
            owner,
            repo,
            branch,
            author_name: config.commit_author_name.clone(),
            author_email: config.commit_author_email.clone(),
        })
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

    /// Batch commit multiple files in a single commit using Git Data API
    pub async fn batch_commit(&self, files: &[PendingFile], message: &str) -> Result<()> {
        if files.is_empty() {
            return Ok(());
        }

        info!(
            "Batch commit to {}/{} branch {} ({} files)",
            self.owner,
            self.repo,
            self.branch,
            files.len()
        );

        let api_base = format!("repos/{}/{}/git", self.owner, self.repo);
        let ref_path = format!("{}/ref/heads/{}", api_base, self.branch);
        info!("Fetching ref: {}", ref_path);

        // 1. Get current branch ref to find HEAD commit
        let ref_response: serde_json::Value = self
            .client
            .get(&ref_path, None::<&()>)
            .await
            .context("Failed to get branch ref")?;

        let head_sha = ref_response["object"]["sha"]
            .as_str()
            .context("Missing HEAD sha")?
            .to_string();

        debug!("Current HEAD: {}", head_sha);

        // 2. Get the tree SHA from HEAD commit
        let commit_response: serde_json::Value = self
            .client
            .get(format!("{}/commits/{}", api_base, head_sha), None::<&()>)
            .await
            .context("Failed to get HEAD commit")?;

        let base_tree_sha = commit_response["tree"]["sha"]
            .as_str()
            .context("Missing tree sha")?
            .to_string();

        debug!("Base tree: {}", base_tree_sha);

        // 3. Create blobs for each file
        let mut tree_entries = Vec::new();
        for file in files {
            let blob_request = CreateBlobRequest {
                content: file.content.clone(),
                encoding: "utf-8".to_string(),
            };

            let blob_response: BlobResponse = self
                .client
                .post(format!("{}/blobs", api_base), Some(&blob_request))
                .await
                .context(format!("Failed to create blob for {}", file.path))?;

            debug!("Created blob for {}: {}", file.path, blob_response.sha);

            tree_entries.push(TreeEntry {
                path: file.path.clone(),
                mode: "100644".to_string(), // regular file
                entry_type: "blob".to_string(),
                sha: blob_response.sha,
            });
        }

        // 4. Create new tree with updated files
        let tree_request = CreateTreeRequest {
            base_tree: base_tree_sha,
            tree: tree_entries,
        };

        let tree_response: TreeResponse = self
            .client
            .post(format!("{}/trees", api_base), Some(&tree_request))
            .await
            .context("Failed to create tree")?;

        debug!("Created tree: {}", tree_response.sha);

        // 5. Create commit
        let author = CommitAuthor {
            name: self.author_name.clone(),
            email: self.author_email.clone(),
        };

        let commit_request = CreateCommitRequest {
            message: message.to_string(),
            tree: tree_response.sha,
            parents: vec![head_sha],
            author: author.clone(),
            committer: author,
        };

        let new_commit: CommitResponse = self
            .client
            .post(format!("{}/commits", api_base), Some(&commit_request))
            .await
            .context("Failed to create commit")?;

        debug!("Created commit: {}", new_commit.sha);

        // 6. Update branch ref to point to new commit
        let update_ref = UpdateRefRequest {
            sha: new_commit.sha.clone(),
        };

        self.client
            .patch::<serde_json::Value, _, _>(
                format!("{}/refs/heads/{}", api_base, self.branch),
                Some(&update_ref),
            )
            .await
            .context("Failed to update branch ref")?;

        info!(
            "Batch committed {} file(s) to {}/{}",
            files.len(),
            self.owner,
            self.repo
        );

        Ok(())
    }
}
