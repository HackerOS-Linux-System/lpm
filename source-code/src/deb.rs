/// Pure-Rust .deb package extractor.
///
/// A .deb file is an `ar` archive containing:
///   debian-binary       – "2.0\n"
///   control.tar.{gz,xz,zst}  – metadata
///   data.tar.{gz,xz,zst,bz2} – actual files
///
/// We extract both tarballs ourselves without calling dpkg.

use anyhow::{bail, Context, Result};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crate::package::Package;

// ─────────────────────────────────────────────────────────────
//  Public API
// ─────────────────────────────────────────────────────────────

pub struct DebPackage {
    pub control:      Package,
    pub control_raw:  String,
    pub data_bytes:   Vec<u8>,
    pub data_compression: Compression,
    /// Files extracted (relative paths from the tarball)
    pub file_list:    Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum Compression { Gz, Xz, Zst, Bz2, None }

impl DebPackage {
    /// Parse a .deb file from raw bytes
    pub fn parse(deb_bytes: &[u8]) -> Result<DebPackage> {
        let mut control_raw  = String::new();
        let mut data_bytes   = Vec::new();
        let mut data_comp    = Compression::None;

        // ar archive: fixed 8-byte magic, then entries
        let magic = b"!<arch>\n";
        if deb_bytes.len() < 8 || &deb_bytes[..8] != magic {
            bail!("Not a valid .deb file (bad ar magic)");
        }

        let mut pos = 8usize;

        while pos + 60 <= deb_bytes.len() {
            // ar header: 60 bytes
            let header = &deb_bytes[pos..pos + 60];
            let name_raw = std::str::from_utf8(&header[0..16])
            .unwrap_or("")
            .trim()
            .trim_end_matches('/');
            let size_str = std::str::from_utf8(&header[48..58])
            .unwrap_or("0")
            .trim();
            let size: usize = size_str.parse().unwrap_or(0);

            pos += 60; // advance past header

            let end = pos + size;
            if end > deb_bytes.len() {
                bail!("Truncated .deb file at member '{}'", name_raw);
            }

            let member_bytes = &deb_bytes[pos..end];

            match name_raw {
                "debian-binary" => {
                    // Just "2.0\n" – ignore
                }
                n if n.starts_with("control.tar") => {
                    let comp = compression_from_name(n);
                    let tar_bytes = decompress_member(member_bytes, comp)
                    .with_context(|| format!("Decompressing {}", n))?;
                    control_raw = extract_control_from_tar(&tar_bytes)
                    .context("Extracting ./control from control tarball")?;
                }
                n if n.starts_with("data.tar") => {
                    data_comp  = compression_from_name(n);
                    data_bytes = member_bytes.to_vec();
                }
                _ => {}
            }

            // ar entries are 2-byte aligned
            pos = end + (end % 2);
        }

        if control_raw.is_empty() {
            bail!("No control member found in .deb");
        }

        let control = Package::parse_block(&control_raw)
        .context("Parsing control file")?;

        // Build file list by peeking into data tarball
        let file_list = list_tar_files(&data_bytes, data_comp)
        .unwrap_or_default();

        Ok(DebPackage {
            control,
            control_raw,
            data_bytes,
            data_compression: data_comp,
            file_list,
        })
    }

    /// Extract all data files into `root` directory.
    /// Returns list of absolute paths written.
    pub fn extract_data(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let tar_bytes = decompress_member(&self.data_bytes, self.data_compression)
        .context("Decompressing data.tar")?;

        extract_tar_to(&tar_bytes, root)
    }

    /// Extract conffiles list from control tarball (if present).
    pub fn conffiles(&self) -> Vec<String> {
        // We'd need to store the control tar – simplified: parse from raw
        // Usually conffiles is listed separately in control.tar/conffiles
        Vec::new()
    }
}

// ─────────────────────────────────────────────────────────────
//  Helpers
// ─────────────────────────────────────────────────────────────

fn compression_from_name(name: &str) -> Compression {
    if name.ends_with(".gz")  { Compression::Gz  }
    else if name.ends_with(".xz")  { Compression::Xz  }
    else if name.ends_with(".zst") { Compression::Zst }
    else if name.ends_with(".bz2") { Compression::Bz2 }
    else                           { Compression::None }
}

fn decompress_member(bytes: &[u8], comp: Compression) -> Result<Vec<u8>> {
    match comp {
        Compression::Gz => {
            let mut dec = flate2::read::GzDecoder::new(bytes);
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        Compression::Xz => {
            let mut dec = xz2::read::XzDecoder::new(bytes);
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        Compression::Zst => {
            let mut dec = zstd::stream::Decoder::new(bytes)?;
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        Compression::Bz2 => {
            let mut dec = bzip2::read::BzDecoder::new(bytes);
            let mut out = Vec::new();
            dec.read_to_end(&mut out)?;
            Ok(out)
        }
        Compression::None => Ok(bytes.to_vec()),
    }
}

fn extract_control_from_tar(tar_bytes: &[u8]) -> Result<String> {
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.to_string_lossy();

        if name == "./control" || name == "control" {
            let mut s = String::new();
            entry.read_to_string(&mut s)?;
            return Ok(s);
        }
    }
    bail!("./control not found in control tarball")
}

fn list_tar_files(bytes: &[u8], comp: Compression) -> Result<Vec<String>> {
    let tar_bytes = decompress_member(bytes, comp)?;
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));
    let mut files = Vec::new();

    for entry in archive.entries()? {
        let entry = entry?;
        let path  = entry.path()?;
        let s     = path.to_string_lossy().to_string();
        // Skip directories
        if !s.ends_with('/') {
            files.push(s);
        }
    }
    Ok(files)
}

fn extract_tar_to(tar_bytes: &[u8], root: &Path) -> Result<Vec<PathBuf>> {
    let mut archive = tar::Archive::new(Cursor::new(tar_bytes));
    let mut written = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_owned();

        // Strip leading ./ from paths
        let rel: PathBuf = path.components()
        .skip_while(|c| matches!(c, std::path::Component::CurDir))
        .collect();

        if rel.as_os_str().is_empty() { continue; }

        let dest = root.join(&rel);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        entry.unpack(&dest)
        .with_context(|| format!("Extracting {:?}", dest))?;

        written.push(dest);
    }

    Ok(written)
}
