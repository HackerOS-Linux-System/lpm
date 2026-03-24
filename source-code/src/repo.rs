use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

use crate::apt_sources::{SourceEntry, SourcesList};

pub struct RepoManager;

impl RepoManager {
    /// Zwraca listę wszystkich repozytoriów wraz z ich indeksami.
    pub fn list() -> Result<Vec<(usize, SourceEntry)>> {
        let sources = SourcesList::load()?;
        Ok(sources.entries.into_iter().enumerate().collect())
    }

    /// Dodaje nowe repozytorium (zapisuje w /etc/apt/sources.list.d/lpm_*.list).
    pub fn add(uri: &str, suite: &str, components: &[String]) -> Result<()> {
        let dir = Path::new("/etc/apt/sources.list.d");
        fs::create_dir_all(dir).context("Cannot create sources.list.d")?;

        let safe_name = format!(
            "lpm_{}_{}.list",
            uri.replace(&['/', ':', '.', '-', '_'][..], "_"),
                                suite.replace(&['/', ':', '.', '-', '_'][..], "_")
        );
        let file = dir.join(safe_name);

        let line = format!("deb {} {} {}\n", uri, suite, components.join(" "));
        fs::write(&file, line)?;

        println!("Repository added to {}", file.display());
        Ok(())
    }

    /// Usuwa repozytorium o podanym ID (indeksie na liście).
    /// Uwaga: to uproszczona wersja – w praktyce wymaga edycji plików .list.
    pub fn remove(_id: usize) -> Result<()> {
        bail!("Removing repositories by ID is not implemented. Please edit /etc/apt/sources.list or /etc/apt/sources.list.d/ manually.");
    }

    /// Włącza repozytorium.
    pub fn enable(_id: usize) -> Result<()> {
        bail!("Enabling repositories by ID is not implemented. Please edit the .list file manually.");
    }

    /// Wyłącza repozytorium.
    pub fn disable(_id: usize) -> Result<()> {
        bail!("Disabling repositories by ID is not implemented. Please edit the .list file manually.");
    }
}
