use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

use crate::apt_sources::{EntryKind, SourceEntry, SourcesList, LPM_SOURCES_TOML, LPM_SOURCES_LIST, LPM_SOURCES_DIR};

pub struct RepoManager;

impl RepoManager {
    /// Zwraca listę wszystkich repozytoriów wraz z ich indeksami.
    pub fn list() -> Result<Vec<(usize, SourceEntry)>> {
        let sources = SourcesList::load()?;
        Ok(sources.entries.into_iter().enumerate().collect())
    }

    /// Dodaje nowe repozytorium do /etc/lpm/sources-list.toml.
    pub fn add(uri: &str, suite: &str, components: &[String]) -> Result<()> {
        std::fs::create_dir_all("/etc/lpm")
        .context("Cannot create /etc/lpm")?;

        let toml_path = Path::new(LPM_SOURCES_TOML);

        // Wczytaj istniejącą treść lub zacznij od zera
        let existing = if toml_path.exists() {
            fs::read_to_string(toml_path).unwrap_or_default()
        } else {
            String::new()
        };

        // Wygeneruj nazwę repo
        let safe_name = format!(
            "{}-{}",
            uri.trim_end_matches('/').split('/').last().unwrap_or("repo"),
                                suite
        );

        // Serializuj nowy blok [[repo]]
        let comp_toml = components.iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");

        let new_block = format!(
            "\n[[repo]]\nname       = \"{}\"\nbaseurl    = \"{}\"\nsuite      = \"{}\"\ncomponents = [{}]\narch       = [\"amd64\"]\nenabled    = true\n",
            safe_name, uri, suite, comp_toml
        );

        let mut content = existing;
        content.push_str(&new_block);

        fs::write(toml_path, &content)?;
        println!(
            "Repository '{}' added to {}",
            safe_name, LPM_SOURCES_TOML
        );
        Ok(())
    }

    /// Usuwa repozytorium o podanym ID.
    pub fn remove(id: usize) -> Result<()> {
        Self::modify_by_id(id, |_entry| None) // None = usuń
    }

    /// Włącza repozytorium o podanym ID.
    pub fn enable(id: usize) -> Result<()> {
        Self::modify_by_id(id, |mut e| {
            e.enabled = true;
            Some(e)
        })
    }

    /// Wyłącza repozytorium o podanym ID.
    pub fn disable(id: usize) -> Result<()> {
        Self::modify_by_id(id, |mut e| {
            e.enabled = false;
            Some(e)
        })
    }

    // ──────────────────────────────────────────────────────────
    //  Wewnętrzny helper: modyfikuj wpis według ID
    // ──────────────────────────────────────────────────────────

    fn modify_by_id<F>(id: usize, transform: F) -> Result<()>
    where
    F: Fn(SourceEntry) -> Option<SourceEntry>,
    {
        let sources  = SourcesList::load()?;
        let entries: Vec<SourceEntry> = sources.entries;

        if id >= entries.len() {
            bail!("No repository with ID {}. Run `lpm repo list` to see available.", id);
        }

        // Operujemy na sources-list.toml jeśli istnieje
        let toml_path = Path::new(LPM_SOURCES_TOML);
        if toml_path.exists() {
            return Self::rewrite_toml(id, &entries, transform);
        }

        // Fallback: sources.list (stary format)
        let list_path = Path::new(LPM_SOURCES_LIST);
        if list_path.exists() {
            return Self::rewrite_list(id, &entries, transform);
        }

        bail!("Cannot find config file to modify. Check /etc/lpm/")
    }

    fn rewrite_toml<F>(id: usize, entries: &[SourceEntry], transform: F) -> Result<()>
    where
    F: Fn(SourceEntry) -> Option<SourceEntry>,
    {
        let toml_path = Path::new(LPM_SOURCES_TOML);
        let mut output = String::new();
        let mut deb_idx = 0usize;

        // Prosta regeneracja TOML na podstawie listy wpisów
        output.push_str("# /etc/lpm/sources-list.toml — managed by lpm\n\n");

        for (i, entry) in entries.iter().enumerate() {
            if entry.kind != EntryKind::Deb {
                continue;
            }

            let current_entry = if i == id {
                match transform(entry.clone()) {
                    Some(e) => e,
                    None    => { deb_idx += 1; continue; } // usuń
                }
            } else {
                entry.clone()
            };

            let name = current_entry.label.as_deref().unwrap_or("repo");
            let comp_toml = current_entry.components.iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", ");
            let arch_toml = if current_entry.arches.is_empty() {
                "\"amd64\"".to_owned()
            } else {
                current_entry.arches.iter()
                .map(|a| format!("\"{}\"", a))
                .collect::<Vec<_>>()
                .join(", ")
            };

            output.push_str(&format!(
                "[[repo]]\nname       = \"{}\"\nbaseurl    = \"{}\"\nsuite      = \"{}\"\ncomponents = [{}]\narch       = [{}]\nenabled    = {}\n\n",
                name,
                current_entry.uri,
                current_entry.suite,
                comp_toml,
                arch_toml,
                current_entry.enabled
            ));
            deb_idx += 1;
        }

        fs::write(toml_path, output)?;
        println!("Updated {}", LPM_SOURCES_TOML);
        Ok(())
    }

    fn rewrite_list<F>(id: usize, entries: &[SourceEntry], transform: F) -> Result<()>
    where
    F: Fn(SourceEntry) -> Option<SourceEntry>,
    {
        let list_path = Path::new(LPM_SOURCES_LIST);
        let mut output = String::new();
        output.push_str("# /etc/lpm/sources.list — managed by lpm\n");

        for (i, entry) in entries.iter().enumerate() {
            let current = if i == id {
                match transform(entry.clone()) {
                    Some(e) => e,
                    None    => continue, // usuń
                }
            } else {
                entry.clone()
            };

            let kind_str = if current.kind == EntryKind::Deb { "deb" } else { "deb-src" };
            let enabled_prefix = if current.enabled { "" } else { "# " };
            let comp_str = current.components.join(" ");
            output.push_str(&format!(
                "{}{} {} {} {}\n",
                enabled_prefix, kind_str, current.uri, current.suite, comp_str
            ));
        }

        fs::write(list_path, output)?;
        println!("Updated {}", LPM_SOURCES_LIST);
        Ok(())
    }
}
