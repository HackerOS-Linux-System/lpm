use anyhow::{bail, Context, Result};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crate::package::Package;

// ─────────────────────────────────────────────────────────────
//  Public types
// ─────────────────────────────────────────────────────────────

pub struct DebPackage {
    pub control:          Package,
    pub control_raw:      String,
    pub data_bytes:       Vec<u8>,
    pub data_compression: Compression,
    /// Regular files only — used for DB file tracking
    pub file_list:        Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Compression { Gz, Xz, Zst, Bz2, None }

impl DebPackage {
    pub fn parse(deb_bytes: &[u8]) -> Result<Self> {
        let magic = b"!<arch>\n";
        if deb_bytes.len() < 8 || &deb_bytes[..8] != magic {
            bail!("Not a valid .deb file (bad ar magic)");
        }

        let mut control_raw = String::new();
        let mut data_bytes  = Vec::new();
        let mut data_comp   = Compression::None;
        let mut pos         = 8usize;

        while pos + 60 <= deb_bytes.len() {
            let header   = &deb_bytes[pos..pos + 60];
            let name_raw = std::str::from_utf8(&header[0..16])
            .unwrap_or("").trim().trim_end_matches('/');
            let size: usize = std::str::from_utf8(&header[48..58])
            .unwrap_or("0").trim().parse().unwrap_or(0);

            pos += 60;
            let end = pos + size;
            if end > deb_bytes.len() {
                bail!("Truncated .deb at ar member '{}'", name_raw);
            }
            let member = &deb_bytes[pos..end];

            match name_raw {
                "debian-binary" => {}
                n if n.starts_with("control.tar") => {
                    let comp  = comp_from_name(n);
                    let tar   = decompress(member, comp)
                    .with_context(|| format!("Decompressing {}", n))?;
                    control_raw = extract_control(&tar)
                    .context("Extracting ./control")?;
                }
                n if n.starts_with("data.tar") => {
                    data_comp  = comp_from_name(n);
                    data_bytes = member.to_vec();
                }
                _ => {}
            }

            pos = end + (end % 2); // ar 2-byte alignment
        }

        if control_raw.is_empty() {
            bail!("No control.tar found in .deb");
        }

        let control   = Package::parse_block(&control_raw).context("Parsing control")?;
        let file_list = list_regular_files(&data_bytes, data_comp).unwrap_or_default();

        Ok(DebPackage { control, control_raw, data_bytes, data_compression: data_comp, file_list })
    }

    /// Extract the data tarball into `root`.
    ///
    /// Returns `(regular_files, all_extracted)`:
    ///   - `regular_files`: only regular files + hard links → stored in DB for removal
    ///   - `all_extracted`:  everything including symlinks → used by fix_alternatives
    pub fn extract_data(&self, root: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        let tar = decompress(&self.data_bytes, self.data_compression)
        .context("Decompressing data.tar")?;
        extract_tar(root, &tar)
    }
}

// ─────────────────────────────────────────────────────────────
//  Decompression
// ─────────────────────────────────────────────────────────────

fn comp_from_name(name: &str) -> Compression {
    if      name.ends_with(".gz")  { Compression::Gz  }
    else if name.ends_with(".xz")  { Compression::Xz  }
    else if name.ends_with(".zst") { Compression::Zst }
    else if name.ends_with(".bz2") { Compression::Bz2 }
    else                           { Compression::None }
}

fn decompress(bytes: &[u8], comp: Compression) -> Result<Vec<u8>> {
    match comp {
        Compression::Gz => {
            let mut d = flate2::read::GzDecoder::new(bytes);
            let mut v = Vec::new(); d.read_to_end(&mut v)?; Ok(v)
        }
        Compression::Xz => {
            let mut d = xz2::read::XzDecoder::new(bytes);
            let mut v = Vec::new(); d.read_to_end(&mut v)?; Ok(v)
        }
        Compression::Zst => {
            let mut d = zstd::stream::Decoder::new(bytes)?;
            let mut v = Vec::new(); d.read_to_end(&mut v)?; Ok(v)
        }
        Compression::Bz2 => {
            let mut d = bzip2::read::BzDecoder::new(bytes);
            let mut v = Vec::new(); d.read_to_end(&mut v)?; Ok(v)
        }
        Compression::None => Ok(bytes.to_vec()),
    }
}

// ─────────────────────────────────────────────────────────────
//  Control extraction
// ─────────────────────────────────────────────────────────────

fn extract_control(tar_bytes: &[u8]) -> Result<String> {
    let mut a = tar::Archive::new(Cursor::new(tar_bytes));
    for entry in a.entries()? {
        let mut e = entry?;
        let name  = e.path()?.to_string_lossy().to_string();
        if name == "./control" || name == "control" {
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            return Ok(s);
        }
    }
    bail!("./control not found in control.tar")
}

// ─────────────────────────────────────────────────────────────
//  File listing (for DB tracking)
// ─────────────────────────────────────────────────────────────

fn list_regular_files(bytes: &[u8], comp: Compression) -> Result<Vec<String>> {
    let tar = decompress(bytes, comp)?;
    let mut archive = tar::Archive::new(Cursor::new(tar));
    let mut files = Vec::new();

    for entry in archive.entries()? {
        let entry = entry?;
        use tar::EntryType;
        if matches!(entry.header().entry_type(), EntryType::Regular | EntryType::Continuous) {
            let s = entry.path()?.to_string_lossy().to_string();
            let s = s.trim_start_matches("./");
            if !s.is_empty() {
                files.push(format!("/{}", s));
            }
        }
    }
    Ok(files)
}

// ─────────────────────────────────────────────────────────────
//  Extraction
// ─────────────────────────────────────────────────────────────

fn extract_tar(root: &Path, tar_bytes: &[u8]) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut archive  = tar::Archive::new(Cursor::new(tar_bytes));
    let mut regular  = Vec::new(); // regular files + hard links
    let mut all_extr = Vec::new(); // everything we touched (incl. symlinks)

    for entry in archive.entries()? {
        let mut entry = entry?;

        let rel: PathBuf = entry.path()?.components()
        .skip_while(|c| matches!(c, std::path::Component::CurDir))
        .collect();
        if rel.as_os_str().is_empty() { continue; }

        let dest = root.join(&rel);

        use tar::EntryType;
        match entry.header().entry_type() {
            EntryType::Directory => {
                std::fs::create_dir_all(&dest)?;
                // Don't track directories
            }

            EntryType::Regular | EntryType::Continuous => {
                if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }
                entry.unpack(&dest)
                .with_context(|| format!("Extracting {:?}", dest))?;
                regular.push(dest.clone());
                all_extr.push(dest);
            }

            EntryType::Symlink => {
                if let Some(target) = entry.link_name()? {
                    if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }
                    // Remove any existing file/symlink at this path
                    let _ = std::fs::remove_file(&dest);
                    // Create the symlink
                    std::os::unix::fs::symlink(&*target, &dest).ok();
                    // Track symlinks so fix_alternatives can inspect them
                    all_extr.push(dest);
                }
            }

            EntryType::Link => {
                // Hard link: unpack creates the hard link
                if let Some(p) = dest.parent() { std::fs::create_dir_all(p)?; }
                entry.unpack(&dest).ok();
                regular.push(dest.clone());
                all_extr.push(dest);
            }

            _ => {
                // char/block devices, FIFOs — skip
            }
        }
    }

    Ok((regular, all_extr))
}
