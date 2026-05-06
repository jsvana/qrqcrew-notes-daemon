use anyhow::Result;
use clap::Parser;
use futures::stream::{self, StreamExt};
use qrqcrew_notes_daemon::nickname_cache::CachedLookup;
use qrqcrew_notes_daemon::qrz::QrzInfo;
use qrqcrew_notes_daemon::{
    Config, CsvFetcher, GitHubClient, GitHubTarget, HtmlFetcher, Member, NicknameCache,
    NotesGenerator, PendingFile, QrzClient,
};
use std::collections::{HashMap, HashSet};
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
                        org.name,
                        pending.path,
                        pending.member_count,
                        pending.target.owner,
                        pending.target.repo
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
        enrich_with_nicknames(
            &mut members,
            qrz,
            nickname_cache,
            &org.name,
            max_concurrent_lookups,
        )
        .await;
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

/// One QRZ result keyed by the queried (roster) callsign.
#[derive(Debug, Clone)]
enum LookupResult {
    Found(QrzInfo),
    NotFound,
    Error,
}

/// Enrich members with QRZ data (current callsign + nickname).
///
/// For every member: look up QRZ once. Apply the canonical `<call>` back
/// onto the member (so retired/aliased roster entries become the operator's
/// current callsign in the generated PoLo notes), and apply `<fname>` as
/// the nickname. After remapping, dedupe by current callsign — if two
/// roster rows resolve to the same operator, the lower member number wins.
async fn enrich_with_nicknames(
    members: &mut Vec<Member>,
    qrz: &QrzClient,
    cache: &Arc<RwLock<NicknameCache>>,
    org_name: &str,
    max_concurrent: usize,
) {
    // Collect roster callsigns and split into cached / uncached.
    let queried_callsigns: Vec<String> = members.iter().map(|m| m.callsign.clone()).collect();

    let mut cache_hits = 0;
    let mut cached_results: HashMap<String, LookupResult> = HashMap::new();
    let mut uncached: Vec<String> = Vec::new();

    {
        let cache_read = cache.read().await;
        for cs in &queried_callsigns {
            match cache_read.get(cs) {
                Some(CachedLookup::Found {
                    current_call,
                    nickname,
                }) => {
                    cache_hits += 1;
                    cached_results.insert(
                        cs.clone(),
                        LookupResult::Found(QrzInfo {
                            current_call,
                            nickname,
                        }),
                    );
                }
                Some(CachedLookup::NotFound) => {
                    cache_hits += 1;
                    cached_results.insert(cs.clone(), LookupResult::NotFound);
                }
                None => uncached.push(cs.clone()),
            }
        }
    }

    let lookups_needed = uncached.len();
    if lookups_needed > 0 {
        info!(
            "[{}] QRZ enrichment: {} cache hits, {} lookups needed (max {} concurrent)",
            org_name, cache_hits, lookups_needed, max_concurrent
        );
    } else {
        info!(
            "[{}] QRZ enrichment: {} cache hits, 0 lookups needed",
            org_name, cache_hits
        );
    }

    // Parallel QRZ lookups for the uncached set.
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let fresh_results: Vec<(String, LookupResult)> = stream::iter(uncached)
        .map(|callsign| {
            let qrz = qrz.clone();
            let semaphore = semaphore.clone();
            let org_name = org_name.to_string();
            async move {
                let _permit = semaphore.acquire().await.unwrap();
                sleep(Duration::from_millis(50)).await;

                let result = match qrz.lookup(&callsign).await {
                    Ok(Some(info)) => {
                        debug!(
                            "[{}] QRZ {} -> current_call={} nickname={:?}",
                            org_name, callsign, info.current_call, info.nickname
                        );
                        LookupResult::Found(info)
                    }
                    Ok(None) => {
                        debug!("[{}] QRZ {} not found", org_name, callsign);
                        LookupResult::NotFound
                    }
                    Err(e) => {
                        warn!("[{}] QRZ lookup failed for {}: {}", org_name, callsign, e);
                        LookupResult::Error
                    }
                };

                (callsign, result)
            }
        })
        .buffer_unordered(max_concurrent)
        .collect()
        .await;

    // Persist new lookups (don't cache transient errors).
    {
        let mut cache_write = cache.write().await;
        for (queried, result) in &fresh_results {
            match result {
                LookupResult::Found(info) => cache_write.insert_found(queried, info),
                LookupResult::NotFound => cache_write.insert_not_found(queried),
                LookupResult::Error => {} // try again next cycle
            }
        }
        if let Err(e) = cache_write.save() {
            warn!("[{}] Failed to save QRZ cache: {}", org_name, e);
        }
    }

    // Build a unified queried -> result map.
    let mut by_queried: HashMap<String, LookupResult> = cached_results;
    for (q, r) in fresh_results {
        by_queried.insert(q, r);
    }

    // Apply: replace member.callsign with QRZ's current_call, set nickname.
    let mut remapped = 0;
    let mut nicknames_found = 0;
    for member in members.iter_mut() {
        match by_queried.get(&member.callsign) {
            Some(LookupResult::Found(info)) => {
                if !info.current_call.eq_ignore_ascii_case(&member.callsign) {
                    info!(
                        "[{}] Remapping {} -> {} (operator's current callsign per QRZ)",
                        org_name, member.callsign, info.current_call
                    );
                    member.callsign = info.current_call.clone();
                    remapped += 1;
                }
                if let Some(nick) = &info.nickname {
                    member.nickname = Some(nick.clone());
                    nicknames_found += 1;
                }
            }
            Some(LookupResult::NotFound) | Some(LookupResult::Error) | None => {}
        }
    }

    // Dedupe: if remap produced two rows with the same callsign, keep the
    // one with the numerically lower member id (typical convention: lower
    // number = older membership). Stable: input order otherwise preserved.
    let dropped = dedupe_by_callsign(members);
    if dropped > 0 {
        info!(
            "[{}] Dropped {} duplicate row(s) after callsign remap",
            org_name, dropped
        );
    }

    info!(
        "[{}] QRZ enrichment complete: {} nicknames found, {} callsigns remapped",
        org_name, nicknames_found, remapped
    );
}

/// Drop duplicate rows that share a callsign after remapping. Returns the
/// number of rows dropped. Within a duplicate group, keeps the row whose
/// `member_id` parses to the smallest integer (or the first row if none
/// parse).
fn dedupe_by_callsign(members: &mut Vec<Member>) -> usize {
    if members.len() < 2 {
        return 0;
    }
    // Find best index per callsign.
    let mut best: HashMap<String, usize> = HashMap::new();
    for (idx, m) in members.iter().enumerate() {
        let key = m.callsign.to_uppercase();
        match best.get(&key) {
            None => {
                best.insert(key, idx);
            }
            Some(&existing) => {
                let cur_n = members[idx].member_id.parse::<u64>().ok();
                let exi_n = members[existing].member_id.parse::<u64>().ok();
                let cur_better = match (cur_n, exi_n) {
                    (Some(a), Some(b)) => a < b,
                    (Some(_), None) => true,
                    _ => false,
                };
                if cur_better {
                    best.insert(key, idx);
                }
            }
        }
    }
    let keep: HashSet<usize> = best.values().copied().collect();
    let original_len = members.len();
    let mut idx = 0;
    members.retain(|_| {
        let k = keep.contains(&idx);
        idx += 1;
        k
    });
    original_len - members.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(callsign: &str, member_id: &str) -> Member {
        Member {
            callsign: callsign.to_string(),
            member_id: member_id.to_string(),
            nickname: None,
        }
    }

    #[test]
    fn dedupe_keeps_lowest_member_id() {
        let mut members = vec![m("W6JY", "100"), m("W6JY", "16"), m("K4MW", "1")];
        let dropped = dedupe_by_callsign(&mut members);
        assert_eq!(dropped, 1);
        assert_eq!(members.len(), 2);
        let w6jy = members.iter().find(|m| m.callsign == "W6JY").unwrap();
        assert_eq!(w6jy.member_id, "16");
    }

    #[test]
    fn dedupe_no_duplicates_is_noop() {
        let mut members = vec![m("W6JY", "16"), m("K4MW", "1")];
        let dropped = dedupe_by_callsign(&mut members);
        assert_eq!(dropped, 0);
        assert_eq!(members.len(), 2);
    }
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
