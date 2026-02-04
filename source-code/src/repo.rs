use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub architecture: String,
    pub filename: String, // Relative path in repo or full URL
    pub size: u64,
    pub sha256: String,
    pub depends: Vec<String>,
    pub provides: Vec<String>,
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            description: String::new(),
            architecture: "amd64".into(),
            filename: String::new(),
            size: 0,
            sha256: String::new(),
            depends: vec![],
            provides: vec![],
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct RepoConfigDefinition {
    pub id: String,
    pub url: String,
    pub dist: String,
    pub components: Vec<String>,
    pub enabled: bool,
}

#[derive(Deserialize, Debug)]
pub struct RepoFile {
    pub repo: RepoConfigDefinition,
}

pub fn get_repos() -> Result<Vec<RepoConfigDefinition>> {
    let mut repos = Vec::new();
    let lpm_path = Path::new("/etc/lpm/repos.d/");

    // 1. Load Custom LPM Repos
    if lpm_path.exists() {
        for entry in walkdir::WalkDir::new(lpm_path) {
            let entry = entry?;
            if entry.path().extension().map_or(false, |e| e == "toml") {
                let content = fs::read_to_string(entry.path())?;
                if let Ok(config) = toml::from_str::<RepoFile>(&content) {
                    if config.repo.enabled {
                        repos.push(config.repo);
                    }
                }
            }
        }
    }

    // 2. Load System APT Repos
    // Check main file
    let apt_sources = Path::new("/etc/apt/sources.list");
    if apt_sources.exists() {
        repos.extend(parse_apt_file(apt_sources)?);
    }

    // Check conf.d
    let apt_sources_d = Path::new("/etc/apt/sources.list.d");
    if apt_sources_d.exists() {
        for entry in walkdir::WalkDir::new(apt_sources_d) {
            let entry = entry?;
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "list" {
                    repos.extend(parse_apt_file(path)?);
                } else if ext == "sources" {
                    repos.extend(parse_deb822_file(path)?);
                }
            }
        }
    }

    Ok(repos)
}

// Old one-line format parser
fn parse_apt_file(path: &Path) -> Result<Vec<RepoConfigDefinition>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut repos = Vec::new();
    // Relaxed Regex: Handles optional space after ']' and flexible spacing
    let re = Regex::new(r"^deb\s+(?:\[.*?\]\s*)?(\S+)\s+(\S+)\s+(.+)$").unwrap();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = re.captures(trimmed) {
            let url = caps.get(1).map_or("", |m| m.as_str()).trim_end_matches('/');
            let dist = caps.get(2).map_or("", |m| m.as_str());
            let components_str = caps.get(3).map_or("", |m| m.as_str());
            let components: Vec<String> = components_str.split_whitespace().map(|s| s.to_string()).collect();

            repos.push(create_repo_def(url, dist, components));
        }
    }
    Ok(repos)
}

// Modern Deb822 format parser (Ubuntu 24.04+)
fn parse_deb822_file(path: &Path) -> Result<Vec<RepoConfigDefinition>> {
    let content = fs::read_to_string(path)?;
    let mut repos = Vec::new();

    // Split by double newlines to separate stanzas
    let stanzas: Vec<&str> = content.split("\n\n").collect();

    for stanza in stanzas {
        let mut props = HashMap::new();
        for line in stanza.lines() {
            if let Some((key, val)) = line.split_once(':') {
                props.insert(key.trim().to_lowercase(), val.trim());
            }
        }

        // Check if it is a binary repo
        if let Some(types) = props.get("types") {
            if !types.contains("deb") { continue; } // Skip deb-src
        } else {
            // If types is missing but we have URIs, assume deb? No, strictly Deb822 requires Types.
            // But let's be safe and skip if unsure.
            continue;
        }

        let uris_str = props.get("uris").unwrap_or(&"");
        let suites_str = props.get("suites").unwrap_or(&"");
        let components_str = props.get("components").unwrap_or(&"");

        let uris: Vec<&str> = uris_str.split_whitespace().collect();
        let suites: Vec<&str> = suites_str.split_whitespace().collect();
        let components: Vec<String> = components_str.split_whitespace().map(|s| s.to_string()).collect();

        // Combinatorial generation: One RepoConfigDefinition per URI+Suite combination
        for uri in uris {
            for suite in &suites {
                repos.push(create_repo_def(uri, suite, components.clone()));
            }
        }
    }

    Ok(repos)
}

fn create_repo_def(url: &str, dist: &str, components: Vec<String>) -> RepoConfigDefinition {
    // Generate a deterministic ID based on URL and Dist
    let mut hasher = Sha256::new();
    hasher.update(format!("{}{}", url, dist));
    let hash_str = hex::encode(hasher.finalize());
    let short_id = &hash_str[0..8];
    let safe_dist = dist.replace('/', "_");

    RepoConfigDefinition {
        id: format!("apt_{}_{}", safe_dist, short_id),
        url: url.trim_end_matches('/').to_string(),
        dist: dist.to_string(),
        components,
        enabled: true,
    }
}

pub fn get_cache_path(repo_id: &str, component: &str) -> PathBuf {
    Path::new("/var/lib/lpm/lists").join(format!("{}_{}_Packages", repo_id, component))
}

pub async fn refresh_metadata() -> Result<()> {
    let repos = get_repos()?;
    if repos.is_empty() {
        println!("Warning: No repositories found in /etc/apt/sources.list or /etc/lpm/repos.d/");
        return Ok(());
    }

    // Ensure directory exists
    if let Err(e) = fs::create_dir_all("/var/lib/lpm/lists") {
        return Err(anyhow::anyhow!("Failed to create cache directory (check permissions?): {}", e));
    }

    println!("Found {} repositories.", repos.len());

    let pb = ProgressBar::new(0);
    pb.set_style(ProgressStyle::default_bar()
    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
    .unwrap()
    .progress_chars("#>-"));

    let mut total_tasks = 0;
    for repo in &repos {
        total_tasks += repo.components.len();
    }
    pb.set_length(total_tasks as u64);

    let mut tasks = Vec::new();
    let client = reqwest::Client::builder()
    .user_agent("Legendary/0.5 (Debian-compatible)")
    .build()?;

    for repo in &repos {
        for comp in &repo.components {
            let url = if repo.dist.ends_with('/') {
                format!("{}/{}/Packages.gz", repo.url, repo.dist)
            } else {
                format!("{}/dists/{}/{}/binary-amd64/Packages.gz", repo.url, repo.dist, comp)
            };

            let dest = get_cache_path(&repo.id, comp);
            let client = client.clone();
            let pb = pb.clone();
            let comp_name = comp.clone();
            let dist_name = repo.dist.clone();

            tasks.push(tokio::spawn(async move {
                pb.set_message(format!("{} {}", dist_name, comp_name));
                match client.get(&url).send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            if let Ok(bytes) = resp.bytes().await {
                                // Try GZIP
                                let mut content = Vec::new();
                                if url.ends_with(".gz") {
                                    let mut gz = flate2::read::GzDecoder::new(&bytes[..]);
                                    if std::io::Read::read_to_end(&mut gz, &mut content).is_ok() {
                                        let _ = fs::write(&dest, content);
                                    } else {
                                        // Fallback
                                        let _ = fs::write(&dest, bytes);
                                    }
                                } else {
                                    let _ = fs::write(&dest, bytes);
                                }
                            }
                        }
                    },
                    Err(_) => {}
                }
                pb.inc(1);
            }));
        }
    }

    for task in tasks {
        let _ = task.await;
    }

    pb.finish_with_message("Done");
    Ok(())
}

pub async fn search(term: &str) -> Result<Vec<PackageMetadata>> {
    let repos = get_repos()?;
    let mut results = Vec::new();
    let term_lower = term.to_lowercase();

    for repo in repos {
        for comp in &repo.components {
            let path = get_cache_path(&repo.id, comp);
            if path.exists() {
                // Removed unused BufReader creation
                let content = fs::read_to_string(&path)?;
                let pkgs = parse_deb_control(&content, &repo.url);

                for pkg in pkgs {
                    if pkg.name.to_lowercase().contains(&term_lower) {
                        results.push(pkg);
                    }
                }
            }
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results.dedup_by(|a, b| a.name == b.name);

    Ok(results)
}

pub fn parse_deb_control(content: &str, repo_url: &str) -> Vec<PackageMetadata> {
    let mut packages = Vec::with_capacity(1000);
    let mut current = PackageMetadata::default();
    let mut active = false;
    let mut last_key = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if line.is_empty() {
            if active {
                if !current.filename.starts_with("http") && !current.filename.is_empty() {
                    current.filename = format!("{}/{}", repo_url, current.filename);
                }
                packages.push(current);
                current = PackageMetadata::default();
                active = false;
            }
            continue;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            if last_key == "Description" {
                current.description.push('\n');
                current.description.push_str(trimmed);
            }
            continue;
        }

        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            active = true;
            last_key = key.to_string();

            match key {
                "Package" => current.name = val.to_string(),
                "Version" => current.version = val.to_string(),
                "Architecture" => current.architecture = val.to_string(),
                "Filename" => current.filename = val.to_string(),
                "Size" => current.size = val.parse().unwrap_or(0),
                "SHA256" => current.sha256 = val.to_string(),
                "Description" => current.description = val.to_string(),
                "Depends" => {
                    current.depends = val.split(',').map(|s| s.trim().to_string()).collect();
                }
                "Provides" => {
                    current.provides = val.split(',').map(|s| s.trim().to_string()).collect();
                }
                _ => {}
            }
        }
    }
    if active {
        if !current.filename.starts_with("http") && !current.filename.is_empty() {
            current.filename = format!("{}/{}", repo_url, current.filename);
        }
        packages.push(current);
    }
    packages
}
