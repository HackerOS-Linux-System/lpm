use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

// ─────────────────────────────────────────────────────────────
//  Paths – lpm obsługuje OBYDWA formaty konfiguracji
// ─────────────────────────────────────────────────────────────

/// Nowy format TOML (priorytet)
pub const LPM_SOURCES_TOML:    &str = "/etc/lpm/sources-list.toml";
/// Stary format APT one-liner
pub const LPM_SOURCES_LIST:    &str = "/etc/lpm/sources.list";
pub const LPM_SOURCES_DIR:     &str = "/etc/lpm/sources.list.d";
/// Fallback do systemowego apt/sources.list
pub const APT_SOURCES_LIST:    &str = "/etc/apt/sources.list";
pub const APT_SOURCES_LIST_D:  &str = "/etc/apt/sources.list.d";

// ─────────────────────────────────────────────────────────────
//  TOML schema
// ─────────────────────────────────────────────────────────────

/// Pojedynczy [[repo]] z sources-list.toml
#[derive(Debug, Deserialize, Clone)]
pub struct TomlRepo {
    pub name:       String,
    pub baseurl:    String,
    pub suite:      String,
    pub components: Vec<String>,
    #[serde(default)]
    pub arch:       Vec<String>,
    #[serde(default = "default_true")]
    pub enabled:    bool,
    pub gpgkey:     Option<String>,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
struct SourcesListToml {
    #[serde(rename = "repo")]
    repos: Vec<TomlRepo>,
}

// ─────────────────────────────────────────────────────────────
//  Unified SourceEntry
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind { Deb, DebSrc }

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub kind:       EntryKind,
    pub uri:        String,
    pub suite:      String,
    pub components: Vec<String>,
    pub arches:     Vec<String>,
    pub signed_by:  Option<String>,
    pub enabled:    bool,
    /// Human label (z pola `name` w TOML, albo wygenerowany)
    pub label:      Option<String>,
}

// ─────────────────────────────────────────────────────────────
//  SourcesList
// ─────────────────────────────────────────────────────────────

pub struct SourcesList {
    pub entries: Vec<SourceEntry>,
}

impl SourcesList {
    pub fn load() -> Result<Self> {
        let mut entries = Vec::new();

        // 1) /etc/lpm/sources-list.toml  (nowy format – priorytet)
        let toml_path = Path::new(LPM_SOURCES_TOML);
        if toml_path.exists() {
            let txt = std::fs::read_to_string(toml_path)?;
            entries.extend(parse_toml_sources(&txt));
        }

        // 2) /etc/lpm/sources.list  (stary format APT)
        let main = Path::new(LPM_SOURCES_LIST);
        if main.exists() {
            let txt = std::fs::read_to_string(main)?;
            entries.extend(parse_sources_list(&txt));
        }

        // 3) /etc/lpm/sources.list.d/*.list
        let dir = Path::new(LPM_SOURCES_DIR);
        if dir.exists() {
            let mut paths: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                ext == "list" || ext == "toml"
            })
            .collect();
            paths.sort();
            for p in paths {
                if let Ok(txt) = std::fs::read_to_string(&p) {
                    let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                    if ext == "toml" {
                        entries.extend(parse_toml_sources(&txt));
                    } else {
                        entries.extend(parse_sources_list(&txt));
                    }
                }
            }
        }

        // 4) Fallback: /etc/apt/sources.list (system apt)
        if entries.is_empty() {
            let apt_main = Path::new(APT_SOURCES_LIST);
            if apt_main.exists() {
                let txt = std::fs::read_to_string(apt_main)?;
                entries.extend(parse_sources_list(&txt));
            }
            let apt_dir = Path::new(APT_SOURCES_LIST_D);
            if apt_dir.exists() {
                let mut paths: Vec<_> = std::fs::read_dir(apt_dir)?
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
        }

        // 5) Jeśli nadal pusto – utwórz domyślną konfigurację
        if entries.is_empty() {
            Self::create_default_sources()?;
            return Self::load();
        }

        Ok(SourcesList { entries })
    }

    fn create_default_sources() -> Result<()> {
        std::fs::create_dir_all("/etc/lpm")?;

        let default_toml = r#"# /etc/lpm/sources-list.toml
        # Domyślna konfiguracja lpm – Debian stable

        [[repo]]
        name       = "debian-stable-main"
        baseurl    = "https://deb.debian.org/debian"
        suite      = "stable"
        components = ["main", "contrib", "non-free", "non-free-firmware"]
        arch       = ["amd64"]
        enabled    = true

        [[repo]]
        name       = "debian-stable-security"
        baseurl    = "https://security.debian.org/debian-security"
        suite      = "stable-security"
        components = ["main", "contrib", "non-free"]
        arch       = ["amd64"]
        enabled    = true

        [[repo]]
        name       = "debian-stable-updates"
        baseurl    = "https://deb.debian.org/debian"
        suite      = "stable-updates"
        components = ["main", "contrib", "non-free"]
        arch       = ["amd64"]
        enabled    = true
        "#;

        let toml_path = Path::new(LPM_SOURCES_TOML);
        if !toml_path.exists() {
            std::fs::write(toml_path, default_toml)?;
            println!("Created default config at {}", LPM_SOURCES_TOML);
        }
        Ok(())
    }

    /// Generuje listę URL-i do pobrania indeksów pakietów.
    pub fn index_urls(&self, arch: &str) -> Vec<IndexUrl> {
        let mut out = Vec::new();

        for entry in &self.entries {
            if !entry.enabled || entry.kind != EntryKind::Deb {
                continue;
            }

            let arch_list: Vec<&str> = if entry.arches.is_empty() {
                vec![arch]
            } else {
                entry.arches.iter().map(|s| s.as_str()).collect()
            };

            for a in &arch_list {
                if entry.components.is_empty() {
                    // Flat repo – np. własne małe repo bez suite/component
                    let url = format!(
                        "{}/Packages",
                        entry.uri.trim_end_matches('/')
                    );
                    out.push(IndexUrl {
                        url,
                        base_uri:  entry.uri.clone(),
                             suite:     entry.suite.clone(),
                             component: String::new(),
                             arch:      a.to_string(),
                             label:     entry.label.clone(),
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
                                 label:     entry.label.clone(),
                        });
                    }
                }
            }
        }

        out
    }

    /// Wszystkie aktywne wpisy deb (do wyświetlenia w `lpm repo list`).
    pub fn active_deb_entries(&self) -> Vec<&SourceEntry> {
        self.entries
        .iter()
        .filter(|e| e.enabled && e.kind == EntryKind::Deb)
        .collect()
    }
}

// ─────────────────────────────────────────────────────────────
//  Parsowanie TOML
// ─────────────────────────────────────────────────────────────

fn parse_toml_sources(content: &str) -> Vec<SourceEntry> {
    // Usuń komentarze inline (toml crate ich nie lubi w starszych wersjach)
    let clean: String = content
    .lines()
    .map(|l| {
        // Nie usuwaj '#' wewnątrz stringów – prosty heurystyk
        if let Some(idx) = l.find(" #") {
            &l[..idx]
        } else {
            l
        }
    })
    .collect::<Vec<_>>()
    .join("\n");

    let parsed: SourcesListToml = match toml::from_str(&clean) {
        Ok(p)  => p,
        Err(e) => {
            eprintln!("Warning: failed to parse sources-list.toml: {}", e);
            return Vec::new();
        }
    };

    parsed.repos.into_iter().map(|r| SourceEntry {
        kind:       EntryKind::Deb,
        uri:        r.baseurl,
        suite:      r.suite,
        components: r.components,
        arches:     r.arch,
        signed_by:  r.gpgkey,
        enabled:    r.enabled,
        label:      Some(r.name),
    }).collect()
}

// ─────────────────────────────────────────────────────────────
//  Parsowanie formatu APT one-liner
// ─────────────────────────────────────────────────────────────

fn parse_sources_list(content: &str) -> Vec<SourceEntry> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(e) = parse_deb822_line(line) {
            out.push(e);
        }
    }
    out
}

fn parse_deb822_line(line: &str) -> Option<SourceEntry> {
    // Obsługa wierszy z "#" wyłączonymi: "# deb ..."
    let (enabled, line) = if line.starts_with("# deb") {
        (false, line.trim_start_matches('#').trim())
    } else {
        (true, line)
    };

    let (kind, rest) = if line.starts_with("deb-src") {
        (EntryKind::DebSrc, line["deb-src".len()..].trim_start())
    } else if line.starts_with("deb") {
        (EntryKind::Deb, line["deb".len()..].trim_start())
    } else {
        return None;
    };

    let (options, rest) = if rest.starts_with('[') {
        let end = rest.find(']')?;
        (Some(&rest[1..end]), rest[end + 1..].trim_start())
    } else {
        (None, rest)
    };

    let mut tokens = rest.split_whitespace();
    let uri        = tokens.next()?.to_owned();
    let suite      = tokens.next()?.to_owned();
    let components: Vec<String> = tokens.map(|s| s.to_owned()).collect();

    let (arches, signed_by) = parse_options(options);

    Some(SourceEntry {
        kind,
         uri,
         suite,
         components,
         arches,
         signed_by,
         enabled,
         label: None,
    })
}

fn parse_options(opts: Option<&str>) -> (Vec<String>, Option<String>) {
    let mut arches    = Vec::new();
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

#[derive(Debug, Clone)]
pub struct IndexUrl {
    pub url:       String,
    pub base_uri:  String,
    pub suite:     String,
    pub component: String,
    pub arch:      String,
    pub label:     Option<String>,
}

// ─────────────────────────────────────────────────────────────
//  Pomocnicze: normalizacja nazwy suite
//  Pozwala używać "trixie", "sid", "forky", "stable", "testing"
//  zamiennie – traktujemy je jako aliasy Debiana.
// ─────────────────────────────────────────────────────────────

pub fn normalize_suite(suite: &str) -> &str {
    match suite {
        "stable"   => "stable",
        "testing"  => "testing",
        "unstable" => "unstable",
        "sid"      => "unstable", // sid == unstable
        other      => other,      // forky, trixie, bookworm – zostawiamy bez zmian
    }
}
