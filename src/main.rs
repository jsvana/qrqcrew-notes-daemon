use anyhow::Result;
use clap::Parser;
use futures::stream::{self, StreamExt};
use qrqcrew_notes_daemon::{
    Config, CsvFetcher, GitHubClient, GitHubTarget, HtmlFetcher, Member, NicknameCache,
    NotesGenerator, PendingFile, QrzClient,
};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

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

    let enabled_orgs: Vec<_> = config.organizations.iter().filter(|o| o.enabled).collect();

    if enabled_orgs.is_empty() {
        warn!("No organizations enabled in config");
        return Ok(());
    }

    info!(
        "Starting callsign notes daemon with {} enabled organization(s)",
        enabled_orgs.len()
    );

    // Initialize QRZ client if configured
    let qrz_client = match &config.qrz {
        Some(qrz_config) if qrz_config.enabled => {
            info!("QRZ lookups enabled");
            Some(QrzClient::new(
                qrz_config.username.clone(),
                qrz_config.password.clone(),
            ))
        }
        Some(_) => {
            info!("QRZ lookups disabled in config");
            None
        }
        None => {
            info!("QRZ not configured, nicknames will not be fetched");
            None
        }
    };

    // Persistent nickname cache (survives daemon restarts)
    let cache_path = config
        .qrz
        .as_ref()
        .and_then(|q| q.cache_path.clone())
        .unwrap_or_else(|| "nickname_cache.json".to_string());

    let nickname_cache = Arc::new(RwLock::new(
        NicknameCache::load(&cache_path).unwrap_or_else(|e| {
            warn!("Failed to load nickname cache: {}, starting fresh", e);
            NicknameCache::load("/dev/null").unwrap() // Empty cache
        }),
    ));

    loop {
        // Run connectivity diagnostics before each sync cycle
        run_connectivity_check().await;

        let mut pending_files = Vec::new();

        // Get max concurrent lookups from config
        let max_concurrent_lookups = config
            .qrz
            .as_ref()
            .and_then(|q| q.max_concurrent_lookups)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_LOOKUPS);

        for org in &enabled_orgs {
            info!("[{}] Starting sync", org.name);
            match prepare_org_update(
                org,
                &config.github,
                cli.dry_run,
                &qrz_client,
                &nickname_cache,
                max_concurrent_lookups,
            )
            .await
            {
                Ok(Some(pending)) => {
                    info!(
                        "[{}] Prepared update for {} ({} members) -> {}/{}",
                        org.name, pending.path, pending.member_count,
                        pending.target.owner, pending.target.repo
                    );
                    pending_files.push(pending);
                }
                Ok(None) => {
                    info!("[{}] No update needed (dry run or empty roster)", org.name);
                }
                Err(e) => {
                    error!("[{}] Sync failed: {}", org.name, e);
                }
            }
        }

        // Group pending files by target repository
        if !pending_files.is_empty() && !cli.dry_run {
            let mut files_by_target: HashMap<GitHubTarget, Vec<PendingFile>> = HashMap::new();
            for file in pending_files {
                files_by_target
                    .entry(file.target.clone())
                    .or_default()
                    .push(file);
            }

            // Batch commit to each target repository
            for (target, files) in files_by_target {
                match GitHubClient::from_target(&target, &config.github) {
                    Ok(client) => {
                        let message = build_commit_message(&files);
                        if let Err(e) = client.batch_commit(&files, &message).await {
                            error!(
                                "Batch commit to {}/{} failed: {:?}",
                                target.owner, target.repo, e
                            );
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to create GitHub client for {}/{}: {}",
                            target.owner, target.repo, e
                        );
                    }
                }
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

fn build_commit_message(files: &[PendingFile]) -> String {
    if files.len() == 1 {
        let f = &files[0];
        format!(
            "Update {} callsign notes ({} members)\n\nGenerated by qrqcrew-notes-daemon",
            f.org_label, f.member_count
        )
    } else {
        let mut msg = String::from("Update callsign notes\n\n");
        for f in files {
            msg.push_str(&format!("- {}: {} members\n", f.org_label, f.member_count));
        }
        msg.push_str("\nGenerated by qrqcrew-notes-daemon");
        msg
    }
}

async fn prepare_org_update(
    org: &qrqcrew_notes_daemon::config::Organization,
    global_github: &qrqcrew_notes_daemon::config::GitHubConfig,
    dry_run: bool,
    qrz_client: &Option<QrzClient>,
    nickname_cache: &Arc<RwLock<NicknameCache>>,
    max_concurrent_lookups: usize,
) -> Result<Option<PendingFile>> {
    // Resolve the effective GitHub target (per-org override or global fallback)
    let target = GitHubTarget::resolve(org.github.as_ref(), global_github);
    // 1. Fetch roster based on source type
    let mut members = match org.source_type.as_str() {
        "html_table" => {
            let callsign_idx = org.callsign_column_index.unwrap_or(1);
            let number_idx = org.number_column_index.unwrap_or(0);
            let fetcher = HtmlFetcher::new(org.roster_url.clone(), callsign_idx, number_idx);
            fetcher.fetch_members().await?
        }
        _ => {
            // Default to CSV
            let callsign_col = org
                .callsign_column
                .clone()
                .unwrap_or_else(|| "Callsign".to_string());
            let number_col = org
                .number_column
                .clone()
                .unwrap_or_else(|| "Number".to_string());
            let fetcher = CsvFetcher::new(
                org.roster_url.clone(),
                callsign_col,
                number_col,
                org.skip_rows,
            );
            fetcher.fetch_members().await?
        }
    };
    info!(
        "[{}] Fetched {} members from roster",
        org.name,
        members.len()
    );

    if members.is_empty() {
        warn!("[{}] No members found in roster, skipping", org.name);
        return Ok(None);
    }

    // 2. Enrich with nicknames from QRZ if available
    if let Some(qrz) = qrz_client {
        enrich_with_nicknames(&mut members, qrz, nickname_cache, &org.name, max_concurrent_lookups).await;
    }

    // 3. Generate notes file
    let generator = NotesGenerator::new(org.emoji.clone(), org.label.clone(), None);
    let content = generator.generate(&members);

    if dry_run {
        info!("[{}] Dry run - would generate:\n{}", org.name, content);
        return Ok(None);
    }

    // 4. Return pending file for batch commit
    Ok(Some(PendingFile {
        path: org.output_file.clone(),
        content,
        org_label: org.label.clone(),
        member_count: members.len(),
        target,
    }))
}

/// Default max concurrent QRZ lookups
const DEFAULT_MAX_CONCURRENT_LOOKUPS: usize = 10;

/// Enrich members with nicknames from QRZ lookups (parallel with rate limiting)
async fn enrich_with_nicknames(
    members: &mut [Member],
    qrz: &QrzClient,
    cache: &Arc<RwLock<NicknameCache>>,
    org_name: &str,
    max_concurrent: usize,
) {
    let mut cache_hits = 0;
    let mut found = 0;

    // First pass: apply cached values and collect uncached callsigns
    let mut uncached_indices = Vec::new();
    {
        let cache_read = cache.read().await;
        for (idx, member) in members.iter_mut().enumerate() {
            if let Some(nickname) = cache_read.get(&member.callsign) {
                member.nickname = nickname.clone();
                cache_hits += 1;
                if nickname.is_some() {
                    found += 1;
                }
            } else {
                uncached_indices.push(idx);
            }
        }
    }

    let lookups_needed = uncached_indices.len();
    if lookups_needed == 0 {
        info!(
            "[{}] QRZ enrichment: {} cache hits, 0 lookups needed, {} nicknames found",
            org_name, cache_hits, found
        );
        return;
    }

    info!(
        "[{}] QRZ enrichment: {} cache hits, {} lookups needed (max {} concurrent)",
        org_name, cache_hits, lookups_needed, max_concurrent
    );

    // Prepare callsigns for parallel lookup
    let callsigns: Vec<String> = uncached_indices
        .iter()
        .map(|&idx| members[idx].callsign.clone())
        .collect();

    // Semaphore for rate limiting concurrent requests
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    // Perform parallel lookups
    let results: Vec<(String, Option<String>)> = stream::iter(callsigns)
        .map(|callsign| {
            let qrz = qrz.clone();
            let semaphore = semaphore.clone();
            let org_name = org_name.to_string();
            async move {
                // Acquire semaphore permit (rate limiting)
                let _permit = semaphore.acquire().await.unwrap();

                // Small delay between requests to avoid hammering QRZ
                sleep(Duration::from_millis(50)).await;

                let nickname = match qrz.lookup_nickname(&callsign).await {
                    Ok(name) => {
                        if name.is_some() {
                            debug!("[{}] Found nickname for {}: {:?}", org_name, callsign, name);
                        }
                        name
                    }
                    Err(e) => {
                        warn!("[{}] QRZ lookup failed for {}: {}", org_name, callsign, e);
                        None
                    }
                };

                (callsign, nickname)
            }
        })
        .buffer_unordered(max_concurrent)
        .collect()
        .await;

    // Apply results to members and update cache
    let mut new_found = 0;
    {
        let mut cache_write = cache.write().await;
        for (callsign, nickname) in &results {
            cache_write.insert(callsign, nickname.clone());
            if nickname.is_some() {
                new_found += 1;
            }
        }
        // Save cache after batch of lookups
        if let Err(e) = cache_write.save() {
            warn!("[{}] Failed to save nickname cache: {}", org_name, e);
        }
    }

    // Build a map for quick lookup
    let results_map: std::collections::HashMap<_, _> = results.into_iter().collect();

    // Apply to members
    for &idx in &uncached_indices {
        if let Some(nickname) = results_map.get(&members[idx].callsign) {
            members[idx].nickname = nickname.clone();
        }
    }

    info!(
        "[{}] QRZ enrichment complete: {} cache hits, {} lookups, {} new nicknames found",
        org_name, cache_hits, lookups_needed, new_found
    );
}

/// Run connectivity diagnostics to help debug network issues
async fn run_connectivity_check() {
    // Test targets: one from each service we use
    let targets = [
        ("Google (DNS)", "google.com:443"),
        ("Google Sheets", "docs.google.com:443"),
        ("GitHub API", "api.github.com:443"),
    ];

    info!("Running connectivity check...");

    for (name, addr) in targets {
        match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addr)).await {
            Ok(Ok(_stream)) => {
                debug!("[connectivity] {} ({}) - OK", name, addr);
            }
            Ok(Err(e)) => {
                let mut error_msg = format!("{}", e);
                if let Some(source) = e.source() {
                    error_msg.push_str(&format!(" -> {}", source));
                }
                warn!("[connectivity] {} ({}) - FAILED: {}", name, addr, error_msg);
            }
            Err(_) => {
                warn!("[connectivity] {} ({}) - TIMEOUT after 10s", name, addr);
            }
        }
    }
}
