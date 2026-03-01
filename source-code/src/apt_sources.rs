use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ─────────────────────────────────────────────────────────────
//  Data types
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind { Deb, DebSrc }

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub kind:        EntryKind,
    pub uri:         String,
    pub suite:       String,
    pub components:  Vec<String>,
    pub arches:      Vec<String>, // empty = use system arch
    pub signed_by:   Option<String>,
    pub enabled:     bool,
}

pub struct SourcesList {
    pub entries: Vec<SourceEntry>,
}

// ─────────────────────────────────────────────────────────────
//  Future: /etc/lpm/sources-list.toml
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct LpmSourcesFile {
    pub repo: Vec<LpmRepo>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LpmRepo {
    pub name:       String,
    pub baseurl:    String,
    pub suite:      String,
    pub components: Vec<String>,
    #[serde(default)]
    pub arch:       Vec<String>,
    #[serde(default = "bool_true")]
    pub enabled:    bool,
    pub gpgkey:     Option<String>,
}

fn bool_true() -> bool { true }

// ─────────────────────────────────────────────────────────────
//  Loading
// ─────────────────────────────────────────────────────────────

impl SourcesList {
    pub fn load() -> Result<Self> {
        // Prefer /etc/lpm/sources-list.toml if it exists
        let lpm_toml = Path::new("/etc/lpm/sources-list.toml");
        if lpm_toml.exists() {
            return Self::load_lpm_toml(lpm_toml);
        }

        // Fall back to APT sources.list
        let mut entries = Vec::new();

        let main = Path::new("/etc/apt/sources.list");
        if main.exists() {
            let txt = std::fs::read_to_string(main)?;
            entries.extend(parse_sources_list(&txt));
        }

        let d = Path::new("/etc/apt/sources.list.d");
        if d.exists() {
            let mut paths: Vec<_> = std::fs::read_dir(d)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |x| x == "list"))
            .collect();
            paths.sort();
            for p in paths {
                if let Ok(txt) = std::fs::read_to_string(&p) {
                    entries.extend(parse_sources_list(&txt));
                }
            }
        }

        Ok(SourcesList { entries })
    }

    fn load_lpm_toml(path: &Path) -> Result<Self> {
        let txt = std::fs::read_to_string(path)?;
        let file: LpmSourcesFile = toml::from_str(&txt)?;

        let entries = file.repo.into_iter()
        .filter(|r| r.enabled)
        .map(|r| SourceEntry {
            kind:       EntryKind::Deb,
            uri:        r.baseurl,
            suite:      r.suite,
            components: r.components,
            arches:     r.arch,
            signed_by:  r.gpgkey,
            enabled:    true,
        })
        .collect();

        Ok(SourcesList { entries })
    }

    /// Generate all Packages index URLs for the given architecture.
    pub fn index_urls(&self, arch: &str) -> Vec<IndexUrl> {
        let mut out = Vec::new();

        for entry in &self.entries {
            if !entry.enabled || entry.kind != EntryKind::Deb { continue; }

            let arch_list: Vec<&str> = if entry.arches.is_empty() {
                vec![arch]
            } else {
                entry.arches.iter().map(|s| s.as_str()).collect()
            };

            for a in &arch_list {
                if entry.components.is_empty() {
                    // flat repo
                    let url = format!("{}/Packages", entry.uri.trim_end_matches('/'));
                    out.push(IndexUrl {
                        url,
                        base_uri: entry.uri.clone(),
                             suite:     entry.suite.clone(),
                             component: String::new(),
                             arch:      a.to_string(),
                    });
                } else {
                    for comp in &entry.components {
                        let base = entry.uri.trim_end_matches('/');
                        let url = format!(
                            "{}/dists/{}/{}/binary-{}/Packages",
                            base, entry.suite, comp, a
                        );
                        out.push(IndexUrl {
                            url,
                            base_uri:  entry.uri.clone(),
                                 suite:     entry.suite.clone(),
                                 component: comp.clone(),
                                 arch:      a.to_string(),
                        });
                    }
                }
            }
        }

        out
    }
}

// ─────────────────────────────────────────────────────────────
//  Parser – sources.list
// ─────────────────────────────────────────────────────────────

fn parse_sources_list(content: &str) -> Vec<SourceEntry> {
    let mut out = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if let Some(e) = parse_deb822_line(line) {
            out.push(e);
        }
    }

    out
}

fn parse_deb822_line(line: &str) -> Option<SourceEntry> {
    // Determine type
    let (kind, rest) = if line.starts_with("deb-src") {
        (EntryKind::DebSrc, line["deb-src".len()..].trim_start())
    } else if line.starts_with("deb") {
        (EntryKind::Deb, line["deb".len()..].trim_start())
    } else {
        return None;
    };

    // Parse optional bracket options: [arch=amd64 signed-by=/path ...]
    let (options, rest) = if rest.starts_with('[') {
        let end = rest.find(']')?;
        (Some(&rest[1..end]), rest[end + 1..].trim_start())
    } else {
        (None, rest)
    };

    let mut tokens = rest.split_whitespace();
    let uri   = tokens.next()?.to_owned();
    let suite = tokens.next()?.to_owned();
    let components: Vec<String> = tokens.map(|s| s.to_owned()).collect();

    let (arches, signed_by) = parse_options(options);

    Some(SourceEntry {
        kind,
         uri,
         suite,
         components,
         arches,
         signed_by,
         enabled: true,
    })
}

fn parse_options(opts: Option<&str>) -> (Vec<String>, Option<String>) {
    let mut arches = Vec::new();
    let mut signed_by = None;

    if let Some(o) = opts {
        for tok in o.split_whitespace() {
            if let Some(v) = tok.strip_prefix("arch=") {
                arches = v.split(',').map(|s| s.to_owned()).collect();
            }
            if let Some(v) = tok.strip_prefix("signed-by=") {
                signed_by = Some(v.to_owned());
            }
        }
    }

    (arches, signed_by)
}

// ─────────────────────────────────────────────────────────────
//  IndexUrl
// ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct IndexUrl {
    pub url:       String,
    pub base_uri:  String,
    pub suite:     String,
    pub component: String,
    pub arch:      String,
}
