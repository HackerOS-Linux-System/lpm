use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::{Client, StatusCode};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::package::Package;

pub const DL_DIR: &str = "/var/cache/lpm/archives";

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

    // Overall bar
    let total_bytes: u64 = packages.iter().filter_map(|p| p.download_size).sum();
    let overall_style = ProgressStyle::with_template(
        "  {spinner:.cyan} Downloading  [{bar:38.cyan/white}] {bytes}/{total_bytes}  {bytes_per_sec}  ETA {eta}"
    )
    .unwrap()
    .progress_chars("▰▰▱");
    let overall = mp.add(ProgressBar::new(total_bytes));
    overall.set_style(overall_style);

    let pkg_style = ProgressStyle::with_template(
        "    {prefix:<30.dim} {bar:30.green/white} {bytes:>9}/{total_bytes:<9}"
    )
    .unwrap()
    .progress_chars("▰▰▱");

    let mut handles = Vec::new();

    for pkg in packages {
        let base_uri = match &pkg.repo_base_uri {
            Some(u) => u.clone(),
            None    => {
                eprintln!("  Warning: no repo URI for {}, skipping download", pkg.name);
                continue;
            }
        };
        let filename = match &pkg.filename {
            Some(f) => f.clone(),
            None    => {
                eprintln!("  Warning: no filename for {}, skipping", pkg.name);
                continue;
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

        let handle = tokio::spawn(async move {
            let result = download_one(&client, &url, &dest, &pb, &overall).await;
            pb.finish_and_clear();
            result.map(|_| DownloadResult { package: pkg, path: dest })
        });
        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await? {
            Ok(r)  => results.push(r),
            Err(e) => eprintln!("  Download error: {:#}", e),
        }
    }

    overall.finish_and_clear();
    mp.clear().ok();

    Ok(results)
}

async fn download_one(
    client:  &HttpClient,
    url:     &str,
    dest:    &Path,
    pb:      &ProgressBar,
    overall: &ProgressBar,
) -> Result<()> {
    // Skip if already cached with correct size
    if let Ok(meta) = std::fs::metadata(dest) {
        // Optimistic: if file exists and is non-zero, use it
        if meta.len() > 0 {
            overall.inc(meta.len());
            return Ok(());
        }
    }

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

    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await
    .with_context(|| format!("Cannot create {:?}", tmp))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
        overall.inc(chunk.len() as u64);
    }
    file.flush().await?;
    drop(file);

    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

pub fn pkg_dest_path(pkg: &Package) -> PathBuf {
    // Use the same naming as apt: name_version_arch.deb
    let safe_ver = pkg.version.replace(':', "%3A").replace('/', "%2F");
    PathBuf::from(DL_DIR)
    .join(format!("{}_{}_{}.deb", pkg.name, safe_ver, pkg.architecture))
}
