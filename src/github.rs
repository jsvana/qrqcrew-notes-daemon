use crate::config::GitHubConfig;
use anyhow::{Context, Result};
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::time::Duration;
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
    client: reqwest::Client,
    token: String,
    owner: String,
    repo: String,
    branch: String,
    author_name: String,
    author_email: String,
}

impl GitHubClient {
    pub fn new(config: &GitHubConfig) -> Result<Self> {
        info!(
            "Building GitHub client: owner={}, repo={}, branch={}, token_len={}",
            config.owner,
            config.repo,
            config.branch,
            config.token.len()
        );

        if config.token.is_empty() {
            anyhow::bail!("GitHub token is empty");
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")?;

        info!("GitHub client built successfully");

        Ok(Self {
            client,
            token: config.token.clone(),
            owner: config.owner.clone(),
            repo: config.repo.clone(),
            branch: config.branch.clone(),
            author_name: config.commit_author_name.clone(),
            author_email: config.commit_author_email.clone(),
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.github.com/{}", path)
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let url = self.api_url(path);
        debug!("GET {}", url);

        let response = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, "qrqcrew-notes-daemon")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .context("Failed to send request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        response.json().await.context("Failed to parse response")
    }

    async fn post<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.api_url(path);
        debug!("POST {}", url);

        let response = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, "qrqcrew-notes-daemon")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(body)
            .send()
            .await
            .context("Failed to send request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        response.json().await.context("Failed to parse response")
    }

    async fn patch<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let url = self.api_url(path);
        debug!("PATCH {}", url);

        let response = self
            .client
            .patch(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, "qrqcrew-notes-daemon")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(body)
            .send()
            .await
            .context("Failed to send request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        Ok(())
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

        // 1. Get current branch ref to find HEAD commit
        let ref_path = format!("{}/ref/heads/{}", api_base, self.branch);
        info!("Fetching ref: {}", ref_path);

        let ref_response: serde_json::Value = self.get(&ref_path).await?;

        let head_sha = ref_response["object"]["sha"]
            .as_str()
            .context("Missing HEAD sha")?
            .to_string();

        debug!("Current HEAD: {}", head_sha);

        // 2. Get the tree SHA from HEAD commit
        let commit_response: serde_json::Value = self
            .get(&format!("{}/commits/{}", api_base, head_sha))
            .await?;

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
                .post(&format!("{}/blobs", api_base), &blob_request)
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
            .post(&format!("{}/trees", api_base), &tree_request)
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
            .post(&format!("{}/commits", api_base), &commit_request)
            .await
            .context("Failed to create commit")?;

        debug!("Created commit: {}", new_commit.sha);

        // 6. Update branch ref to point to new commit
        let update_ref = UpdateRefRequest {
            sha: new_commit.sha.clone(),
        };

        self.patch(
            &format!("{}/refs/heads/{}", api_base, self.branch),
            &update_ref,
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
