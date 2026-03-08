use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::{Client, StatusCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::package::Package;

pub const DL_DIR: &str = "/var/cache/lpm/archives";

/// Maximum number of concurrent downloads.
/// Prevents "Too many open files" (EMFILE / os error 24).
/// The kernel default ulimit is usually 1024; we stay well below that
/// while still being fast (8 parallel streams ~ 100 MB/s on a good link).
const MAX_CONCURRENT: usize = 8;

/// How many times to retry a failed download before giving up.
const MAX_RETRIES: usize = 3;

/// Delay between retries (seconds).
const RETRY_DELAY_SECS: u64 = 2;

const USER_AGENT: &str = concat!(
    "lpm/", env!("CARGO_PKG_VERSION"),
    " (Legendary Package Manager; +https://github.com/HackerOS-Linux-System/lpm/)"
);

// ─────────────────────────────────────────────────────────────
//  HttpClient
// ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct HttpClient {
    inner: Client,
}

impl HttpClient {
    pub fn new() -> Self {
        let inner = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(20))
            .tcp_keepalive(Duration::from_secs(30))
            .pool_max_idle_per_host(4)
            .gzip(true)
            .deflate(true)
            .build()
            .expect("Failed to build HTTP client");
        HttpClient { inner }
    }

    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.inner.get(url).send().await
            .with_context(|| format!("GET {}", url))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("HTTP {} for {}", status, url);
        }
        Ok(resp.bytes().await?.to_vec())
    }

    pub async fn get_text(&self, url: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.get_bytes(url).await?).to_string())
    }
}

// ─────────────────────────────────────────────────────────────
//  Download a list of packages
//
//  - Concurrency limited to MAX_CONCURRENT via semaphore
//  - Each download retried up to MAX_RETRIES times
//  - ALL packages must download successfully; any failure is FATAL
//    (we never proceed to install with a partial package set)
// ─────────────────────────────────────────────────────────────

pub struct DownloadResult {
    pub package: Package,
    pub path:    PathBuf,
}

pub async fn download_packages(
    client:   &HttpClient,
    packages: &[Package],
) -> Result<Vec<DownloadResult>> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    std::fs::create_dir_all(DL_DIR)
        .context("Cannot create download cache dir")?;

    let mp = MultiProgress::new();

    // Overall progress bar
    let total_bytes: u64 = packages.iter().filter_map(|p| p.download_size).sum();
    let overall_style = ProgressStyle::with_template(
        "  {spinner:.cyan} Downloading  [{bar:38.cyan/white}] {bytes}/{total_bytes}  {bytes_per_sec}  ETA {eta}"
    )
    .unwrap()
    .progress_chars("▰▰▱");
    let overall = mp.add(ProgressBar::new(total_bytes));
    overall.set_style(overall_style);

    let pkg_style = ProgressStyle::with_template(
        "    {prefix:<36.dim} {bar:28.green/white} {bytes:>9}/{total_bytes:<9}"
    )
    .unwrap()
    .progress_chars("▰▰▱");

    // Semaphore: at most MAX_CONCURRENT downloads open simultaneously
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));

    let mut handles = Vec::new();

    for pkg in packages {
        let base_uri = match &pkg.repo_base_uri {
            Some(u) => u.clone(),
            None => {
                // This should not happen after solver resolution
                bail!("Package {} has no repository URI — run `lpm update` first", pkg.name);
            }
        };
        let filename = match &pkg.filename {
            Some(f) => f.clone(),
            None => {
                bail!("Package {} has no filename in metadata — run `lpm update` first", pkg.name);
            }
        };

        let url  = format!("{}/{}", base_uri.trim_end_matches('/'), filename);
        let dest = pkg_dest_path(pkg);

        let pb = mp.add(ProgressBar::new(pkg.download_size.unwrap_or(0)));
        pb.set_style(pkg_style.clone());
        pb.set_prefix(format!("{} {}", pkg.name, pkg.version));

        let client  = client.clone();
        let overall = overall.clone();
        let pkg     = pkg.clone();
        let sem     = Arc::clone(&sem);

        let handle: tokio::task::JoinHandle<Result<DownloadResult>> =
            tokio::spawn(async move {
                // Acquire permit — blocks when MAX_CONCURRENT reached
                let _permit = sem.acquire().await
                    .expect("Semaphore closed");

                let result = download_with_retry(&client, &url, &dest, &pb, &overall).await;
                pb.finish_and_clear();

                result.map(|_| DownloadResult { package: pkg, path: dest })
            });

        handles.push(handle);
    }

    // Collect results — fail HARD if ANY download failed
    let mut results  = Vec::new();
    let mut failures = Vec::new();

    for handle in handles {
        match handle.await {
            Ok(Ok(r))  => results.push(r),
            Ok(Err(e)) => failures.push(format!("{:#}", e)),
            Err(e)     => failures.push(format!("Task panic: {}", e)),
        }
    }

    overall.finish_and_clear();
    mp.clear().ok();

    if !failures.is_empty() {
        // Print all failures clearly, then bail
        eprintln!();
        eprintln!("  {} download(s) failed:", failures.len());
        for f in &failures {
            eprintln!("    ✗ {}", f);
        }
        eprintln!();
        bail!(
            "{} package(s) could not be downloaded. \
             Transaction aborted — nothing was installed.\n\
             Fix your network connection or try again.",
            failures.len()
        );
    }

    Ok(results)
}

// ─────────────────────────────────────────────────────────────
//  Download one file with retry
// ─────────────────────────────────────────────────────────────

async fn download_with_retry(
    client:  &HttpClient,
    url:     &str,
    dest:    &Path,
    pb:      &ProgressBar,
    overall: &ProgressBar,
) -> Result<()> {
    // Check cache first — if file exists and is non-zero, skip download
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.len() > 0 {
            pb.inc(meta.len());
            overall.inc(meta.len());
            return Ok(());
        }
    }

    let mut last_err = anyhow::anyhow!("No attempts made");

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(RETRY_DELAY_SECS)).await;
            pb.reset();
        }

        match download_one(client, url, dest, pb, overall).await {
            Ok(())  => return Ok(()),
            Err(e)  => {
                last_err = e;
                // Only retry on network errors, not 404
                // (last_err message contains "404" for not found)
                if last_err.to_string().contains("404") {
                    break;
                }
            }
        }
    }

    Err(last_err)
}

async fn download_one(
    client:  &HttpClient,
    url:     &str,
    dest:    &Path,
    pb:      &ProgressBar,
    overall: &ProgressBar,
) -> Result<()> {
    let resp = client.inner.get(url).send().await
        .with_context(|| format!("GET {}", url))?;

    if resp.status() == StatusCode::NOT_FOUND {
        bail!("404 Not Found: {}", url);
    }
    if !resp.status().is_success() {
        bail!("HTTP {} for {}", resp.status(), url);
    }

    let content_len = resp.content_length().unwrap_or(0);
    pb.set_length(content_len);

    // Write to .part file, rename on success (atomic)
    let tmp  = dest.with_extension("part");
    // Remove stale .part from a previous failed attempt
    let _ = tokio::fs::remove_file(&tmp).await;
    let mut file = tokio::fs::File::create(&tmp).await
        .with_context(|| format!("Cannot create {:?}", tmp))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Stream error during download")?;
        file.write_all(&chunk).await?;
        let n = chunk.len() as u64;
        pb.inc(n);
        overall.inc(n);
    }
    file.flush().await?;
    drop(file);

    tokio::fs::rename(&tmp, dest).await
        .with_context(|| format!("Cannot rename {:?} → {:?}", tmp, dest))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

pub fn pkg_dest_path(pkg: &Package) -> PathBuf {
    let safe_ver = pkg.version.replace(':', "%3A").replace('/', "%2F");
    PathBuf::from(DL_DIR)
        .join(format!("{}_{}_{}.deb", pkg.name, safe_ver, pkg.architecture))
}
