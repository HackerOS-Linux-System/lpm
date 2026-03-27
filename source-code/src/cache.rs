use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apt_sources::{IndexUrl, SourcesList};
use crate::download::HttpClient;
use crate::package::Package;

pub const LISTS_DIR: &str = "/var/lib/lpm/lists";
pub const CACHE_DIR: &str = "/var/cache/lpm";

pub struct PackageCache {
    by_name: HashMap<String, Package>,
    all:     HashMap<String, Package>,
}

impl PackageCache {
    pub fn empty() -> Self {
        PackageCache {
            by_name: HashMap::new(),
            all:     HashMap::new(),
        }
    }

    pub fn load() -> Result<Self> {
        let mut cache = Self::empty();
        let dir = Path::new(LISTS_DIR);
        if !dir.exists() {
            eprintln!(
                "{}",
                "Warning: package cache directory does not exist. Run `lpm update` first.".yellow()
            );
            return Ok(cache);
        }

        let mut loaded_files   = 0usize;
        let mut total_packages = 0usize;

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path  = entry.path();
            if path.extension().map_or(false, |e| e == "pkgs") {
                let content = match std::fs::read_to_string(&path) {
                    Ok(c)  => c,
                    Err(e) => {
                        eprintln!("Warning: could not read {}: {}", path.display(), e);
                        continue;
                    }
                };
                let base_uri = extract_base_uri_comment(&content);
                let pkgs     = Package::parse_index(&content);
                let count    = pkgs.len();
                total_packages += count;
                loaded_files   += 1;
                for mut pkg in pkgs {
                    if pkg.repo_base_uri.is_none() {
                        pkg.repo_base_uri = base_uri.clone();
                    }
                    cache.ingest(pkg);
                }
            }
        }

        if loaded_files == 0 {
            eprintln!(
                "{}",
                "Warning: no package index files found. Run `lpm update` first.".yellow()
            );
        } else {
            println!(
                "Loaded {} packages from {} index files.",
                total_packages.to_string().green(),
                     loaded_files.to_string().cyan()
            );
        }

        Ok(cache)
    }

    fn ingest(&mut self, pkg: Package) {
        let all_key = format!("{}:{}:{}", pkg.name, pkg.architecture, pkg.version);
        self.all.insert(all_key, pkg.clone());

        let existing_newer = self.by_name.get(&pkg.name).map_or(false, |ex| {
            crate::package::version_cmp(&ex.version, &pkg.version)
            == std::cmp::Ordering::Greater
        });
        if !existing_newer {
            self.by_name.insert(pkg.name.clone(), pkg);
        }
    }

    // ──────────────────────────────────────────────────────────
    //  update – pobiera indeksy ze wszystkich aktywnych repo
    // ──────────────────────────────────────────────────────────

    pub async fn update(sources: &SourcesList, client: &HttpClient) -> Result<()> {
        let arch = detect_arch();
        let urls = sources.index_urls(&arch);

        if urls.is_empty() {
            anyhow::bail!(
                "No repositories configured.\n\
Check /etc/lpm/sources-list.toml or /etc/lpm/sources.list"
            );
        }

        std::fs::create_dir_all(LISTS_DIR)
        .context("Cannot create /var/lib/lpm/lists")?;

        let mp = MultiProgress::new();
        let spinner_style = ProgressStyle::with_template(
            "  {spinner:.cyan}  {prefix:<45.bold} {wide_msg}",
        )
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]);

        let mut handles = Vec::new();

        for url_info in urls {
            let label = url_info.label.clone().unwrap_or_else(|| {
                format!("{}/{} [{}]", url_info.suite, url_info.component, url_info.arch)
            });
            let pb = mp.add(ProgressBar::new_spinner());
            pb.set_style(spinner_style.clone());
            pb.set_prefix(label);
            pb.set_message("connecting...");
            pb.enable_steady_tick(Duration::from_millis(80));

            let client   = client.clone();
            let base_uri = url_info.base_uri.clone();

            let handle = tokio::spawn(async move {
                let result = fetch_index(&client, &url_info).await;
                (url_info, base_uri, pb, result)
            });
            handles.push(handle);
        }

        let mut ok_count  = 0usize;
        let mut err_count = 0usize;

        for handle in handles {
            let (url_info, base_uri, pb, result) = handle.await?;

            match result {
                Ok(content) => {
                    let stored = format!("# lpm-base-uri: {}\n{}", base_uri, content);
                    let fname  = url_to_cache_name(&url_info.url);
                    let dest   = PathBuf::from(LISTS_DIR).join(format!("{}.pkgs", fname));
                    std::fs::write(&dest, &stored)?;

                    let count = Package::parse_index(&content).len();
                    pb.finish_with_message(format!(
                        "{} — {} packages",
                        "OK".green().bold(),
                                                   count.to_string().cyan()
                    ));
                    ok_count += 1;
                }
                Err(e) => {
                    pb.finish_with_message(format!(
                        "{} — {}",
                        "FAILED".red().bold(),
                                                   e.to_string().dimmed()
                    ));
                    err_count += 1;
                }
            }
        }

        mp.clear().ok();
        println!(
            "  {} Updated {}, {} failed.",
            "●".cyan().bold(),
                 ok_count.to_string().green().bold(),
                 err_count.to_string().red().bold()
        );

        Ok(())
    }

    // ──────────────────────────────────────────────────────────
    //  Query API
    // ──────────────────────────────────────────────────────────

    pub fn get(&self, name: &str) -> Option<&Package> {
        self.by_name.get(name)
    }

    pub fn get_exact(&self, name: &str, version: &str, arch: &str) -> Option<&Package> {
        let key = format!("{}:{}:{}", name, arch, version);
        self.all.get(&key)
    }

    pub fn search(&self, query: &str) -> Vec<&Package> {
        let q = query.to_lowercase();
        let mut results: Vec<&Package> = self
        .by_name
        .values()
        .filter(|p| {
            p.name.to_lowercase().contains(&q)
            || p.description_short
            .as_ref()
            .map_or(false, |d| d.to_lowercase().contains(&q))
        })
        .collect();

        results.sort_by(|a, b| {
            let a_exact  = a.name.to_lowercase() == q;
            let b_exact  = b.name.to_lowercase() == q;
            if a_exact != b_exact {
                return b_exact.cmp(&a_exact);
            }
            let a_starts = a.name.to_lowercase().starts_with(&q);
            let b_starts = b.name.to_lowercase().starts_with(&q);
            if a_starts != b_starts {
                return b_starts.cmp(&a_starts);
            }
            a.name.cmp(&b.name)
        });

        results
    }

    pub fn all_packages(&self) -> Vec<&Package> {
        let mut v: Vec<&Package> = self.by_name.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }
}

// ─────────────────────────────────────────────────────────────
//  fetch_index – próbuje kolejno: .xz → .gz → .bz2 → bez kompresji
// ─────────────────────────────────────────────────────────────

async fn fetch_index(client: &HttpClient, info: &IndexUrl) -> Result<String> {
    // Debian używa Packages.xz, Packages.gz lub Packages (plain).
    // Próbujemy w kolejności malejącej wydajności.
    let suffixes: &[&str] = &[".xz", ".gz", ".bz2", ""];

    let mut last_err = anyhow::anyhow!("no attempts");

    for &suffix in suffixes {
        let url = format!("{}{}", info.url, suffix);
        match client.get_bytes(&url).await {
            Ok(bytes) => {
                match decompress(&bytes, suffix) {
                    Ok(text) => return Ok(text),
                    Err(e)   => {
                        last_err = anyhow::anyhow!("Decompression failed for {}: {}", url, e);
                        continue;
                    }
                }
            }
            Err(_) => continue,
        }
    }

    // Sprawdź czy URL jest dostępny w ogóle (lepszy komunikat błędu)
    anyhow::bail!(
        "All variants failed for {}  (tried: {}.xz, {}.gz, {}.bz2, {})\n  {}",
                  info.url, info.url, info.url, info.url, info.url, last_err
    )
}

fn decompress(bytes: &[u8], suffix: &str) -> Result<String> {
    match suffix {
        ".gz" => {
            let mut dec = flate2::read::GzDecoder::new(bytes);
            let mut s = String::new();
            dec.read_to_string(&mut s)?;
            Ok(s)
        }
        ".bz2" => {
            let mut dec = bzip2::read::BzDecoder::new(bytes);
            let mut s = String::new();
            dec.read_to_string(&mut s)?;
            Ok(s)
        }
        ".xz" => {
            let mut dec = xz2::read::XzDecoder::new(bytes);
            let mut s = String::new();
            dec.read_to_string(&mut s)?;
            Ok(s)
        }
        _ => Ok(String::from_utf8_lossy(bytes).to_string()),
    }
}

fn url_to_cache_name(url: &str) -> String {
    url.chars()
    .map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            c
        } else {
            '_'
        }
    })
    .take(120)
    .collect()
}

fn extract_base_uri_comment(content: &str) -> Option<String> {
    content
    .lines()
    .find(|l| l.starts_with("# lpm-base-uri:"))
    .map(|l| l["# lpm-base-uri:".len()..].trim().to_owned())
}

// ─────────────────────────────────────────────────────────────
//  detect_arch
// ─────────────────────────────────────────────────────────────

pub fn detect_arch() -> String {
    // Najpierw dpkg (najbardziej wiarygodne na Debianie)
    if let Ok(out) = std::process::Command::new("dpkg")
        .args(["--print-architecture"])
        .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !s.is_empty() {
                    return s;
                }
            }
        }

        // Fallback: uname -m
        if let Ok(out) = std::process::Command::new("uname").arg("-m").output() {
            if out.status.success() {
                let m = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return match m.as_str() {
                    "x86_64"          => "amd64",
                    "aarch64" | "arm64" => "arm64",
                    "armv7l"          => "armhf",
                    "i686" | "i386"   => "i386",
                    "riscv64"         => "riscv64",
                    other             => other,
                }
                .to_owned();
            }
        }

        // Kompilacja statyczna
        if cfg!(target_arch = "x86_64")  { return "amd64".into(); }
        if cfg!(target_arch = "aarch64") { return "arm64".into(); }
        "amd64".into()
}
