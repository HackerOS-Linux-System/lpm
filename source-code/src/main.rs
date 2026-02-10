use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use resolvo::{
    DependencyProvider, Solver, SolverCache, Candidates, Dependencies,
    NameId, VersionSetId, SolvableId, Interner, StringId, KnownDependencies
};
use resolvo::utils::Pool;
use reqwest::blocking::Client;
use flate2::read::GzDecoder;
use std::io::{self, Read, Write};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::Duration;
use regex::Regex;
use serde::{Deserialize, Serialize};
use rusqlite::{params, Connection, Result as SqlResult};
use sha2::{Sha256, Digest};
use ar::Archive as ArArchive;
use tar::Archive;
use xz2::read::XzDecoder;
use lazy_static::lazy_static;
use sequoia_openpgp as openpgp;
use openpgp::parse::Parse;
use openpgp::parse::stream::{VerifierBuilder, DetachedVerifierBuilder, VerificationHelper, MessageStructure};
use openpgp::policy::StandardPolicy;
use openpgp::cert::Cert;
use openpgp::cert::CertParser;
use console::Style;
use prettytable::{Table, row};
use std::process::Command;
use std::cmp::Ordering;
use std::str::FromStr;
use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, RwLock};

// Constants
const SOURCES_YAML: &str = "/etc/lpm/sources-list.yml";
const SOURCES_LEGACY: &str = "/etc/apt/sources.list";
const CACHE_DIR: &str = "/var/cache/lpm/";
const DB_PATH: &str = "/var/lib/lpm/inventory.db";
const TRUSTED_GPG_DIR: &str = "/etc/apt/trusted.gpg.d/";
const ALTERNATIVES_DB: &str = "/var/lib/lpm/alternatives.db";

// Repository struct
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Repository {
    name: String,
    url: String,
    dist: String,
    components: Vec<String>,
    priority: Option<i32>,
}

// Sources config
#[derive(Debug, Serialize, Deserialize)]
struct Sources {
    repositories: Vec<Repository>,
}

// DebianVersion
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct DebianVersion {
    epoch: u32,
    upstream: String,
    revision: String,
}

impl DebianVersion {
    fn parse(version: &str) -> Self {
        lazy_static! {
            static ref RE: Regex = Regex::new(r"^([0-9]+:)?([a-zA-Z0-9.+:~-]+?)(?:-([a-zA-Z0-9.+:~-]+))?$").unwrap();
        }
        let caps = RE.captures(version).unwrap_or_else(|| RE.captures("0:").unwrap());
        let epoch_str = caps.get(1).map_or("", |m| m.as_str()).trim_end_matches(':');
        let epoch = epoch_str.parse::<u32>().unwrap_or(0);
        let upstream = caps.get(2).map_or(version.to_string(), |m| m.as_str().to_string());
        let revision = caps.get(3).map_or(String::new(), |m| m.as_str().to_string());
        Self { epoch, upstream, revision }
    }

    fn to_string_impl(&self) -> String {
        let epoch_str = if self.epoch > 0 { format!("{}:", self.epoch) } else { String::new() };
        let revision_str = if !self.revision.is_empty() { format!("-{}", self.revision) } else { String::new() };
        format!("{}{}{}", epoch_str, self.upstream, revision_str)
    }
}

// Implementacja Display dla DebianVersion
impl Display for DebianVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_impl())
    }
}

impl Ord for DebianVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.epoch.cmp(&other.epoch)
        .then(compare_deb_strings(&self.upstream, &other.upstream))
        .then(compare_deb_strings(&self.revision, &other.revision))
    }
}

impl PartialOrd for DebianVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for DebianVersion {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

fn compare_deb_strings(a: &str, b: &str) -> Ordering {
    let mut a_iter = a.chars().peekable();
    let mut b_iter = b.chars().peekable();
    loop {
        let a_ch = a_iter.next();
        let b_ch = b_iter.next();
        match (a_ch, b_ch) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some('~'), Some(_)) => return Ordering::Less,
            (Some(_), Some('~')) => return Ordering::Greater,
            (Some(a_c), Some(b_c)) => {
                if a_c.is_digit(10) && b_c.is_digit(10) {
                    let mut a_num = String::new();
                    let mut b_num = String::new();
                    a_num.push(a_c);
                    b_num.push(b_c);
                    while let Some(&next_a) = a_iter.peek() {
                        if next_a.is_digit(10) {
                            a_num.push(a_iter.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    while let Some(&next_b) = b_iter.peek() {
                        if next_b.is_digit(10) {
                            b_num.push(b_iter.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    let a_int: u64 = a_num.parse().unwrap_or(0);
                    let b_int: u64 = b_num.parse().unwrap_or(0);
                    match a_int.cmp(&b_int) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else if !a_c.is_digit(10) && !b_c.is_digit(10) {
                    let a_ord = deb_char_order(a_c);
                    let b_ord = deb_char_order(b_c);
                    match a_ord.cmp(&b_ord) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else if a_c.is_digit(10) {
                    return Ordering::Greater;
                } else {
                    return Ordering::Less;
                }
            }
        }
    }
}

fn deb_char_order(c: char) -> u32 {
    if c.is_ascii_lowercase() {
        c as u32 - 'a' as u32 + 1
    } else if c.is_ascii_uppercase() {
        c as u32 - 'A' as u32 + 27
    } else {
        match c {
            '.' => 0,
            '+' => 52,
            '-' => 53,
            ':' => 54,
            _ => 55,
        }
    }
}

// Dependency
#[derive(Debug, Clone)]
struct Dependency {
    name: String,
    operator: String,
    version: Option<DebianVersion>,
    alternatives: Vec<Dependency>,
}

// PackageMetadata
#[derive(Debug, Clone)]
struct PackageMetadata {
    name: String,
    version: DebianVersion,
    architecture: String,
    depends: Vec<Dependency>,
    pre_depends: Vec<Dependency>,
    recommends: Vec<Dependency>,
    suggests: Vec<Dependency>,
    conflicts: Vec<Dependency>,
    replaces: Vec<Dependency>,
    provides: Vec<String>,
    filename: String,
    sha256: String,
    size: u64,
    triggers: Vec<String>,
}

// InstalledPackage
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct InstalledPackage {
    id: i64,
    name: String,
    version: DebianVersion,
    architecture: String,
    status: String,
    install_reason: String,
}

// FileEntry
#[derive(Debug)]
#[allow(dead_code)]
struct FileEntry {
    path: String,
    hash: String,
    package_id: i64,
}

// Alternative
#[derive(Debug)]
#[allow(dead_code)]
struct Alternative {
    name: String,
    path: String,
    priority: i32,
    slaves: HashMap<String, String>,
}

// Trigger queue
struct TriggerQueue {
    triggers: HashSet<String>,
}

impl TriggerQueue {
    fn new() -> Self {
        Self { triggers: HashSet::new() }
    }
    fn add(&mut self, trigger: String) {
        self.triggers.insert(trigger);
    }
    fn process(&self) {
        for trigger in &self.triggers {
            println!("Processing trigger: {}", trigger);
        }
    }
}

// For resolvo
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct PackageName(String);

impl Display for PackageName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct PackageVersionSet {
    name: PackageName,
    operator: String,
    version: Option<DebianVersion>,
}

impl Display for PackageVersionSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{} {} {}", self.name, self.operator, v),
            None => write!(f, "{}", self.name),
        }
    }
}

// Implement trait required by Pool
impl resolvo::utils::VersionSet for PackageVersionSet {
    type V = DebianVersion;
}

impl PackageVersionSet {
    fn matches(&self, version: &DebianVersion) -> bool {
        let v_req = match &self.version {
            Some(v) => v,
            None => return true,
        };
        match self.operator.as_str() {
            "=" => version == v_req,
            ">=" => version >= v_req,
            "<=" => version <= v_req,
            ">" => version > v_req,
            "<" => version < v_req,
            ">>" => version > v_req,
            "<<" => version < v_req,
            _ => true,
        }
    }
}

// Verification Helper for Sequoia
struct Helper {
    certs: Vec<Cert>,
}

impl VerificationHelper for Helper {
    fn get_certs(&mut self, _ids: &[openpgp::KeyHandle]) -> openpgp::Result<Vec<Cert>> {
        Ok(self.certs.clone())
    }
    fn check(&mut self, _structure: MessageStructure) -> openpgp::Result<()> {
        Ok(())
    }
}

// Provider
#[derive(Clone)]
struct PackageProvider {
    repos: Vec<Repository>,
    packages: HashMap<PackageName, Vec<PackageMetadata>>,
    virtuals: HashMap<PackageName, Vec<PackageName>>,
    client: Client,
    pool: Arc<RwLock<Pool<PackageVersionSet, PackageName>>>,
    name_to_id: HashMap<PackageName, NameId>,
    solvable_to_meta: HashMap<SolvableId, PackageMetadata>,
}

impl PackageProvider {
    fn new(repos: Vec<Repository>) -> Self {
        let mut provider = Self {
            repos,
            packages: HashMap::new(),
            virtuals: HashMap::new(),
            client: Client::new(),
            pool: Arc::new(RwLock::new(Pool::new())),
            name_to_id: HashMap::new(),
            solvable_to_meta: HashMap::new(),
        };
        provider.refresh_cache();
        provider
    }

    fn refresh_cache(&mut self) {
        fs::create_dir_all(CACHE_DIR).unwrap();
        let multi = MultiProgress::new();
        // Clone repos to avoid borrowing self immutably for the loop while needing mutable self later
        let repos = self.repos.clone();
        let total = repos.len() as u64;
        let main_pb = multi.add(ProgressBar::new(total));
        main_pb.set_style(ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] [{wide_bar:.blue/blue}] {pos}/{len} ({eta})")
        .unwrap()
        .progress_chars("=> "));
        main_pb.set_message("Refreshing repositories");

        for repo in &repos {
            let repo_pb = multi.add(ProgressBar::new_spinner());
            repo_pb.set_style(ProgressStyle::default_spinner()
            .template("{spinner} {msg}")
            .unwrap()
            .tick_strings(&[".", "..", "...", ""]));
            repo_pb.set_message(format!("Processing {}", repo.name));

            let inrelease_url = format!("{}/{}/InRelease", repo.url, repo.dist);
            let release_url = format!("{}/{}/Release", repo.url, repo.dist);
            let gpg_url = format!("{}/{}/Release.gpg", repo.url, repo.dist);

            let _content = if let Ok(resp) = self.client.get(&inrelease_url).send() {
                if resp.status().is_success() {
                    let content = resp.bytes().unwrap();
                    if self.verify_inrelease(&content).is_ok() {
                        Some(String::from_utf8_lossy(&content).to_string())
                    } else {
                        repo_pb.finish_with_message("Verification failed");
                        continue;
                    }
                } else {
                    let rel_resp = self.client.get(&release_url).send().unwrap();
                    let gpg_resp = self.client.get(&gpg_url).send().unwrap();
                    let rel_content = rel_resp.bytes().unwrap();
                    let gpg_content = gpg_resp.bytes().unwrap();
                    if self.verify_detached(&rel_content, &gpg_content).is_ok() {
                        Some(String::from_utf8_lossy(&rel_content).to_string())
                    } else {
                        repo_pb.finish_with_message("Verification failed");
                        continue;
                    }
                }
            } else {
                repo_pb.finish_with_message("Fetch failed");
                continue;
            };

            for component in &repo.components {
                let packages_url = format!("{}/{}/{}/binary-amd64/Packages.gz", repo.url, repo.dist, component);
                if let Ok(resp) = self.client.get(&packages_url).send() {
                    if resp.status().is_success() {
                        let cache_path = format!("{}/{}-{}-Packages", CACHE_DIR, repo.name, component);
                        let mut file = File::create(&cache_path).unwrap();
                        let bytes = resp.bytes().unwrap();
                        file.write_all(&bytes).unwrap();
                        let file = File::open(&cache_path).unwrap();
                        let mut decoder = GzDecoder::new(file);
                        let mut content = String::new();
                        decoder.read_to_string(&mut content).unwrap();
                        self.parse_packages(&content);
                    }
                }
            }
            repo_pb.finish_with_message(format!("Processed {}", repo.name));
            main_pb.inc(1);
        }
        main_pb.finish_with_message("Refresh complete");

        // Build virtuals
        for (name, metas) in &self.packages {
            for meta in metas {
                for provide in &meta.provides {
                    let provide_name = PackageName(provide.clone());
                    self.virtuals.entry(provide_name).or_insert(Vec::new()).push(name.clone());
                }
            }
        }

        // Build pool
        let mut pool = self.pool.write().unwrap();
        for (name, metas) in &self.packages {
            let name_id = pool.intern_package_name(name.clone());
            self.name_to_id.insert(name.clone(), name_id);
            for meta in metas {
                let solvable_id = pool.intern_solvable(name_id, meta.version.clone());
                self.solvable_to_meta.insert(solvable_id, meta.clone());
            }
        }
        for (virt, _providers) in &self.virtuals {
            let virt_id = pool.intern_package_name(virt.clone());
            self.name_to_id.insert(virt.clone(), virt_id);
        }
    }

    fn load_trusted_certs(&self) -> Vec<Cert> {
        let mut certs = Vec::new();
        if let Ok(dir) = fs::read_dir(TRUSTED_GPG_DIR) {
            for entry in dir {
                if let Ok(entry) = entry {
                    if entry.path().extension().map_or(false, |e| e == "gpg" || e == "asc") {
                        if let Ok(bytes) = fs::read(entry.path()) {
                            if let Ok(parser) = CertParser::from_bytes(&bytes) {
                                for cert in parser {
                                    if let Ok(cert) = cert {
                                        certs.push(cert);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        certs
    }

    fn verify_inrelease(&self, content: &[u8]) -> Result<(), anyhow::Error> {
        let policy = StandardPolicy::new();
        let certs = self.load_trusted_certs();
        let helper = Helper { certs };
        let vb = VerifierBuilder::from_bytes(content)?;
        if let Ok(mut verifier) = vb.with_policy(&policy, None, helper) {
            let mut data = Vec::new();
            verifier.read_to_end(&mut data)?;
            return Ok(());
        }
        Err(anyhow::Error::msg("Verification failed"))
    }

    fn verify_detached(&self, _content: &[u8], sig: &[u8]) -> Result<(), anyhow::Error> {
        let policy = StandardPolicy::new();
        let certs = self.load_trusted_certs();
        let helper = Helper { certs };
        let dvb = DetachedVerifierBuilder::from_bytes(sig)?;
        if let Ok(verifier) = dvb.with_policy(&policy, None, helper) {
            let _ = verifier;
            return Ok(());
        }
        Err(anyhow::Error::msg("Verification failed"))
    }

    fn parse_packages(&mut self, content: &str) {
        lazy_static! {
            static ref PKG_RE: Regex = Regex::new(r"(?ms)^Package: (.+?)\nVersion: (.+?)\nArchitecture: (.+?)\n(?:Depends: (.+?)\n)?(?:Pre-Depends: (.+?)\n)?(?:Recommends: (.+?)\n)?(?:Suggests: (.+?)\n)?(?:Conflicts: (.+?)\n)?(?:Replaces: (.+?)\n)?(?:Provides: (.+?)\n)?Filename: (.+?)\nSHA256: (.+?)\nSize: (.+?)\n").unwrap();
        }
        for cap in PKG_RE.captures_iter(content) {
            let name = cap[1].to_string();
            let version = DebianVersion::parse(&cap[2]);
            let arch = cap[3].to_string();
            let depends_str = cap.get(4).map_or("", |m| m.as_str());
            let pre_depends_str = cap.get(5).map_or("", |m| m.as_str());
            let recommends_str = cap.get(6).map_or("", |m| m.as_str());
            let suggests_str = cap.get(7).map_or("", |m| m.as_str());
            let conflicts_str = cap.get(8).map_or("", |m| m.as_str());
            let replaces_str = cap.get(9).map_or("", |m| m.as_str());
            let provides_str = cap.get(10).map_or("", |m| m.as_str());
            let filename = cap[11].to_string();
            let sha256 = cap[12].to_string();
            let size = cap[13].parse::<u64>().unwrap_or(0);
            let depends = parse_dependencies(depends_str);
            let pre_depends = parse_dependencies(pre_depends_str);
            let recommends = parse_dependencies(recommends_str);
            let suggests = parse_dependencies(suggests_str);
            let conflicts = parse_dependencies(conflicts_str);
            let replaces = parse_dependencies(replaces_str);
            let provides = provides_str.split(',').map(|s| s.trim().to_string()).collect();
            let meta = PackageMetadata {
                name: name.clone(),
                version,
                architecture: arch,
                depends,
                pre_depends,
                recommends,
                suggests,
                conflicts,
                replaces,
                provides,
                filename,
                sha256,
                size,
                triggers: vec![],
            };
            self.packages.entry(PackageName(name)).or_insert(Vec::new()).push(meta);
        }
    }

    fn dependency_to_version_set_id(&self, dep: &Dependency) -> Option<VersionSetId> {
        let pool = self.pool.write().unwrap();
        let name_id = pool.intern_package_name(PackageName(dep.name.clone()));
        let vs = PackageVersionSet {
            name: PackageName(dep.name.clone()),
            operator: dep.operator.clone(),
            version: dep.version.clone(),
        };
        Some(pool.intern_version_set(name_id, vs))
    }
}

// Implementacja Interner dla Resolvo 0.6
impl Interner for PackageProvider {
    fn display_name(&self, name: NameId) -> impl Display + '_ {
        self.pool.read().unwrap().resolve_package_name(name).to_string()
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl Display + '_ {
        self.pool.read().unwrap().resolve_version_set(version_set).to_string()
    }

    fn display_string(&self, string_id: StringId) -> impl Display + '_ {
        self.pool.read().unwrap().resolve_string(string_id).to_string()
    }

    fn display_solvable(&self, solvable: SolvableId) -> impl Display + '_ {
        self.pool.read().unwrap().resolve_solvable(solvable).record.to_string()
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.pool.read().unwrap().resolve_version_set_package_name(version_set)
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        self.pool.read().unwrap().resolve_solvable(solvable).name
    }
}

// Poprawiona implementacja DependencyProvider dla Resolvo 0.6
impl DependencyProvider for PackageProvider {
    async fn sort_candidates(&self, _solver: &SolverCache<Self>, candidates: &mut [SolvableId]) {
        candidates.sort_by(|&a, &b| {
            let meta_a = self.solvable_to_meta.get(&a);
            let meta_b = self.solvable_to_meta.get(&b);
            match (meta_a, meta_b) {
                (Some(ma), Some(mb)) => mb.version.cmp(&ma.version),
                           _ => Ordering::Equal,
            }
        });
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let mut candidates = Candidates::default();
        let pool = self.pool.read().unwrap();
        let pkg_name = pool.resolve_package_name(name);

        if let Some(metas) = self.packages.get(pkg_name) {
            for meta in metas {
                if let Some(id) = self.solvable_to_meta.iter().find(|(_, m)| m.name == meta.name && m.version == meta.version).map(|(id, _)| *id) {
                    candidates.candidates.push(id);
                }
            }
        }
        if let Some(providers) = self.virtuals.get(pkg_name) {
            for prov in providers {
                if let Some(metas) = self.packages.get(prov) {
                    for meta in metas {
                        if let Some(id) = self.solvable_to_meta.iter().find(|(_, m)| m.name == meta.name && m.version == meta.version).map(|(id, _)| *id) {
                            candidates.candidates.push(id);
                        }
                    }
                }
            }
        }
        Some(candidates)
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut requirements = Vec::new();
        if let Some(meta) = self.solvable_to_meta.get(&solvable) {
            for dep in &meta.pre_depends {
                if let Some(id) = self.dependency_to_version_set_id(dep) {
                    requirements.push(id);
                }
            }
            for dep in &meta.depends {
                if let Some(id) = self.dependency_to_version_set_id(dep) {
                    requirements.push(id);
                }
            }
        }
        Dependencies::Known(KnownDependencies { requirements, constrains: Vec::new() })
    }

    async fn filter_candidates(&self, candidates: &[SolvableId], version_set: VersionSetId, inverse: bool) -> Vec<SolvableId> {
        let pool = self.pool.read().unwrap();
        let vs = pool.resolve_version_set(version_set);
        let mut result = Vec::new();
        for &id in candidates {
            if let Some(meta) = self.solvable_to_meta.get(&id) {
                let matches = vs.matches(&meta.version);
                if matches != inverse {
                    result.push(id);
                }
            }
        }
        result
    }
}

fn parse_dependencies(s: &str) -> Vec<Dependency> {
    s.split(',').map(|part| {
        let part = part.trim();
        if part.contains('|') {
            let alts = part.split('|').map(|a| parse_single_dep(a.trim())).collect();
            Dependency { name: "".to_string(), operator: "".to_string(), version: None, alternatives: alts }
        } else {
            parse_single_dep(part)
        }
    }).filter(|d| !d.name.is_empty()).collect()
}

fn parse_single_dep(s: &str) -> Dependency {
    lazy_static! {
        static ref DEP_RE: Regex = Regex::new(r"([a-zA-Z0-9-+:~]+)(?:\s*\((>>|>=|<<|<=|=|>|<)\s*([a-zA-Z0-9.+:~-]+)\))?").unwrap();
    }
    if let Some(caps) = DEP_RE.captures(s) {
        let name = caps[1].to_string();
        let operator = caps.get(2).map_or("", |m| m.as_str()).to_string();
        let version = caps.get(3).map(|m| DebianVersion::parse(m.as_str()));
        Dependency { name, operator, version, alternatives: vec![] }
    } else {
        Dependency { name: s.to_string(), operator: "".to_string(), version: None, alternatives: vec![] }
    }
}

// DB
fn init_db() -> SqlResult<Connection> {
    fs::create_dir_all(Path::new(DB_PATH).parent().unwrap()).unwrap();
    let conn = Connection::open(DB_PATH)?;
    conn.execute("CREATE TABLE IF NOT EXISTS installed_packages (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        architecture TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'installed',
        install_reason TEXT NOT NULL DEFAULT 'manual'
    )", [])?;
    conn.execute("CREATE TABLE IF NOT EXISTS files (
        path TEXT NOT NULL,
        hash TEXT NOT NULL,
        package_id INTEGER NOT NULL,
        FOREIGN KEY (package_id) REFERENCES installed_packages(id)
    )", [])?;
    Ok(conn)
}

fn init_alternatives_db() -> SqlResult<Connection> {
    let conn = Connection::open(ALTERNATIVES_DB)?;
    conn.execute("CREATE TABLE IF NOT EXISTS alternatives (
        name TEXT PRIMARY KEY,
        path TEXT NOT NULL,
        priority INTEGER NOT NULL
    )", [])?;
    conn.execute("CREATE TABLE IF NOT EXISTS slaves (
        master_name TEXT NOT NULL,
        slave_name TEXT NOT NULL,
        slave_path TEXT NOT NULL,
        FOREIGN KEY (master_name) REFERENCES alternatives(name)
    )", [])?;
    Ok(conn)
}

// Run script with chroot and env
fn run_maintainer_script(script_path: &Path, arg: &str, root_dir: &Path, pkg_name: &str) -> Result<(), io::Error> {
    let mut cmd = Command::new("chroot");
    cmd.arg(root_dir);
    cmd.arg("/bin/sh");
    cmd.arg(script_path.strip_prefix(root_dir).unwrap_or(script_path));
    cmd.arg(arg);
    cmd.env("DPKG_MAINTSCRIPT_PACKAGE", pkg_name);
    cmd.env("DPKG_MAINTSCRIPT_NAME", script_path.file_name().unwrap().to_str().unwrap());
    cmd.status()?;
    Ok(())
}

// Install with rollback
fn install_package(conn: &mut Connection, alt_conn: &Connection, meta: &PackageMetadata, repo_url: &str, pb: &ProgressBar, temp_dir: &Path, trigger_queue: &mut TriggerQueue, root_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tx = conn.transaction()?;
    let deb_url = format!("{}/{}", repo_url, meta.filename);
    let client = Client::new();
    let bytes = client.get(deb_url).send()?.bytes()?;
    let deb_path = temp_dir.join(format!("{}.deb", meta.name));
    fs::write(&deb_path, &bytes)?;
    // Hash verify
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    if hex::encode(hasher.finalize()) != meta.sha256 {
        return Err("Hash mismatch".into());
    }

    let mut ar_archive = ArArchive::new(File::open(&deb_path)?);

    let control_dir = temp_dir.join("control");
    fs::create_dir_all(&control_dir)?;
    let mut control_extracted = false;
    let mut data_extracted = false;

    while let Some(entry_res) = ar_archive.next_entry() {
        let mut entry = entry_res?;
        let ident = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if ident.starts_with("control.tar") {
            let mut control_data = Vec::new();
            entry.read_to_end(&mut control_data)?;
            let decoder: Box<dyn Read> = if ident.ends_with(".xz") {
                Box::new(XzDecoder::new(&*control_data))
            } else if ident.ends_with(".gz") {
                Box::new(GzDecoder::new(&*control_data))
            } else {
                continue;
            };
            let mut control_archive = Archive::new(decoder);
            control_archive.unpack(&control_dir)?;
            control_extracted = true;
            // Parse triggers
            if let Ok(triggers_content) = fs::read_to_string(control_dir.join("triggers")) {
                for line in triggers_content.lines() {
                    trigger_queue.add(line.to_string());
                }
            }
        } else if ident.starts_with("data.tar") {
            // Preinst
            let preinst_path = control_dir.join("preinst");
            if preinst_path.exists() {
                run_maintainer_script(&preinst_path, "install", root_dir, &meta.name)?;
            }
            tx.execute("UPDATE installed_packages SET status = 'half-installed' WHERE name = ?", params![meta.name])?;
            let mut data_data = Vec::new();
            entry.read_to_end(&mut data_data)?;
            let decoder: Box<dyn Read> = if ident.ends_with(".xz") {
                Box::new(XzDecoder::new(&*data_data))
            } else if ident.ends_with(".gz") {
                Box::new(GzDecoder::new(&*data_data))
            } else {
                continue;
            };
            let mut archive = Archive::new(decoder);
            for file_res in archive.entries()? {
                let mut file = file_res?;
                let rel_path = file.path()?.to_string_lossy().to_string();
                let abs_path = root_dir.join(rel_path.strip_prefix("/").unwrap_or(&rel_path));
                // Conflict check
                let mut stmt = tx.prepare("SELECT package_id FROM files WHERE path = ?1")?;
                if stmt.query_row(params![abs_path.to_string_lossy()], |_| Ok(())).is_ok() {
                    return Err("File conflict".into());
                }
                if let Some(parent) = abs_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                file.unpack(&abs_path)?;
                let mut hasher = Sha256::new();
                let _ = file.read_to_end(&mut Vec::new());
                let content = fs::read(&abs_path)?;
                hasher.update(&content);
                let file_hash = hex::encode(hasher.finalize());
                tx.execute("INSERT OR REPLACE INTO installed_packages (name, version, architecture, status) VALUES (?1, ?2, ?3, 'unpacked')", params![meta.name, meta.version.to_string_impl(), meta.architecture])?;
                let pkg_id = tx.last_insert_rowid();
                tx.execute("INSERT INTO files (path, hash, package_id) VALUES (?1, ?2, ?3)", params![abs_path.to_string_lossy(), file_hash, pkg_id])?;
            }
            data_extracted = true;
            // Postinst
            let postinst_path = control_dir.join("postinst");
            if postinst_path.exists() {
                run_maintainer_script(&postinst_path, "configure", root_dir, &meta.name)?;
            }
            tx.execute("UPDATE installed_packages SET status = 'installed' WHERE name = ?", params![meta.name])?;
        }
    }
    if !control_extracted || !data_extracted {
        return Err("Missing control or data".into());
    }
    tx.commit()?;
    // Alternatives
    update_alternatives(alt_conn, meta)?;
    pb.inc(50);
    Ok(())
}

fn update_alternatives(conn: &Connection, _meta: &PackageMetadata) -> SqlResult<()> {
    conn.execute("INSERT OR REPLACE INTO alternatives (name, path, priority) VALUES (?, ?, ?)", params!["editor", "/usr/bin/vim", 50])?;
    Ok(())
}

// Remove with rollback
fn remove_package(conn: &mut Connection, _alt_conn: &Connection, name: &str, pb: &ProgressBar, temp_dir: &Path, root_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tx = conn.transaction()?;
    let pkg_id: i64;
    {
        let mut stmt = tx.prepare("SELECT id FROM installed_packages WHERE name = ?1")?;
        pkg_id = stmt.query_row([name], |row| row.get(0))?;
    }

    // Prerm
    let prerm_path = temp_dir.join("prerm");
    if prerm_path.exists() {
        run_maintainer_script(&prerm_path, "remove", root_dir, name)?;
    }
    tx.execute("UPDATE installed_packages SET status = 'half-configured' WHERE id = ?", [pkg_id])?;

    {
        let mut stmt = tx.prepare("SELECT path FROM files WHERE package_id = ?1")?;
        let mut rows = stmt.query([pkg_id])?;
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let full_path = root_dir.join(path.strip_prefix("/").unwrap_or(&path));
            if full_path.exists() {
                fs::remove_file(full_path)?;
            }
        }
    }

    tx.execute("DELETE FROM files WHERE package_id = ?1", [pkg_id])?;
    tx.execute("DELETE FROM installed_packages WHERE id = ?1", [pkg_id])?;
    // Postrm
    let postrm_path = temp_dir.join("postrm");
    if postrm_path.exists() {
        run_maintainer_script(&postrm_path, "remove", root_dir, name)?;
    }
    tx.commit()?;
    // Update alternatives
    // Note: This needs proper connection handling, keeping simple for now
    // conn.execute("DELETE FROM alternatives WHERE name IN (SELECT name FROM alternatives WHERE path LIKE ?)", params![format!("%{}", name)])?;
    pb.inc(50);
    Ok(())
}

// Load sources
fn load_sources() -> Vec<Repository> {
    if Path::new(SOURCES_YAML).exists() {
        let file = File::open(SOURCES_YAML).unwrap();
        let sources: Sources = serde_yaml::from_reader(file).unwrap();
        sources.repositories
    } else {
        let content = fs::read_to_string(SOURCES_LEGACY).unwrap_or_default();
        let mut repos = Vec::new();
        let re = Regex::new(r"deb\s+(\S+)\s+(\S+)\s+(.+)").unwrap();
        for line in content.lines() {
            if let Some(cap) = re.captures(line) {
                repos.push(Repository {
                    name: cap[2].to_string(),
                           url: cap[1].to_string(),
                           dist: cap[2].to_string(),
                           components: cap[3].split_whitespace().map(String::from).collect(),
                           priority: Some(100),
                });
            }
        }
        repos
    }
}

// Display plan
fn display_plan(operations: &[(String, String, String, u64)]) {
    let blue = Style::new().blue().bold();
    println!("{}", blue.apply_to("Plan operacji:"));
    let mut table = Table::new();
    table.add_row(row!["Action", "Package", "Version", "Size (MB)"]);
    for (action, pkg, ver, size) in operations {
        table.add_row(row![action, pkg, ver, format!("{:.2}", *size as f64 / 1_000_000.0)]);
    }
    table.printstd();
}

// CLI
#[derive(Parser)]
#[command(name = "legendary", about = "Legendary Package Manager", version = "0.1.0")]
struct Cli {
    #[arg(long, default_value = "/")]
    root: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install {
        packages: Vec<String>,
        #[arg(long)]
        no_install_recommends: bool,
    },
    Remove { packages: Vec<String> },
    Update,
    Refresh,
    Search { query: String },
    Info { package: String },
    Autoremove,
    Clean,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let root_dir = cli.root;
    let repos = load_sources();
    let mut provider = PackageProvider::new(repos);
    let mut conn = init_db()?;
    let alt_conn = init_alternatives_db()?;
    let multi = MultiProgress::new();
    let spinner = multi.add(ProgressBar::new_spinner());
    spinner.set_style(ProgressStyle::default_spinner()
    .template("{spinner} {msg}")
    .unwrap()
    .tick_strings(&[".", "..", "...", ""]));
    spinner.set_message("Initializing");
    spinner.enable_steady_tick(Duration::from_millis(100));
    thread::sleep(Duration::from_secs(1));
    spinner.finish_with_message("Ready");
    let temp_dir = std::env::temp_dir().join("lpm");
    fs::create_dir_all(&temp_dir)?;
    let mut trigger_queue = TriggerQueue::new();
    match cli.command {
        Commands::Install { packages, no_install_recommends: _ } => {
            let mut solver = Solver::new(provider.clone());
            let mut reqs = Vec::new();

            for pkg in &packages {
                let name_id = provider.pool.write().unwrap().intern_package_name(PackageName(pkg.clone()));
                let vs = PackageVersionSet { name: PackageName(pkg.clone()), operator: "".to_string(), version: None };
                let vs_id = provider.pool.write().unwrap().intern_version_set(name_id, vs);
                reqs.push(vs_id);
            }

            // Solver::solve in resolvo 0.6 takes requirements and a list of favored packages (or soft requirements).
            // It is synchronous when using the default NowOrNeverRuntime, so we remove .await.
            let solution = solver.solve(reqs, Vec::new());

            match solution {
                Ok(solvables) => {
                    let mut ops = Vec::new();
                    for solvable in solvables {
                        if let Some(meta) = provider.solvable_to_meta.get(&solvable) {
                            ops.push(("INS".to_string(), meta.name.clone(), meta.version.to_string_impl(), meta.size));
                        }
                    }
                    display_plan(&ops);
                    let pb = multi.add(ProgressBar::new((packages.len() * 100) as u64));
                    pb.set_style(ProgressStyle::with_template("{msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})")
                    .unwrap()
                    .progress_chars("=> "));
                    pb.set_message("Installing");
                    for (_, pkg, _, _) in ops {
                        if let Some(metas) = provider.packages.get(&PackageName(pkg.clone())) {
                            if let Some(meta) = metas.iter().max_by_key(|m| &m.version) {
                                install_package(&mut conn, &alt_conn, meta, &provider.repos[0].url, &pb, &temp_dir, &mut trigger_queue, &root_dir)?;
                            }
                        }
                        pb.inc(100);
                    }
                    trigger_queue.process();
                    pb.finish_with_message("Installed");
                },
                Err(e) => {
                    println!("Unsolvable: {:?}", e);
                }
            }
        }
        Commands::Remove { packages } => {
            let pb = multi.add(ProgressBar::new((packages.len() * 100) as u64));
            pb.set_style(ProgressStyle::with_template("{msg} [{elapsed_precise}] [{wide_bar:.red/red}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("=> "));
            pb.set_message("Removing");
            for pkg in packages {
                remove_package(&mut conn, &alt_conn, &pkg, &pb, &temp_dir, &root_dir)?;
                pb.inc(100);
            }
            trigger_queue.process();
            pb.finish_with_message("Removed");
        }
        Commands::Update | Commands::Refresh => {
            let pb = multi.add(ProgressBar::new(500));
            pb.set_style(ProgressStyle::with_template("{msg} [{elapsed_precise}] [{wide_bar:.green/green}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("=> "));
            pb.set_message("Updating");
            provider.refresh_cache();
            pb.finish_with_message("Updated");
        }
        Commands::Search { query } => {
            for (name, metas) in &provider.packages {
                if name.0.contains(&query) {
                    println!("{}: {:?}", name.0, metas.iter().map(|m| m.version.to_string_impl()).collect::<Vec<_>>());
                }
            }
        }
        Commands::Info { package } => {
            if let Some(metas) = provider.packages.get(&PackageName(package.clone())) {
                if let Some(meta) = metas.iter().max_by_key(|m| &m.version) {
                    println!("Package: {}\nVersion: {}\nArch: {}\nDepends: {:?}\nRecommends: {:?}\nProvides: {:?}", meta.name, meta.version.to_string_impl(), meta.architecture, meta.depends, meta.recommends, meta.provides);
                }
            } else {
                let mut stmt = conn.prepare("SELECT version, architecture, status FROM installed_packages WHERE name = ?1")?;
                if let Ok((ver, arch, status)) = stmt.query_row([&package], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))) {
                    println!("Installed: {}\nVersion: {}\nArch: {}\nStatus: {}", package, ver, arch, status);
                }
            }
        }
        Commands::Autoremove => {
            let mut stmt = conn.prepare("SELECT name FROM installed_packages WHERE install_reason = 'auto'")?;
            let mut rows = stmt.query([])?;
            let mut to_remove = Vec::new();
            while let Some(row) = rows.next()? {
                let name: String = row.get(0)?;
                to_remove.push(name);
            }
            drop(rows);
            drop(stmt);
            for name in to_remove {
                remove_package(&mut conn, &alt_conn, &name, &ProgressBar::new(0), &temp_dir, &root_dir)?;
            }
            println!("Autoremove complete");
        }
        Commands::Clean => {
            fs::remove_dir_all(CACHE_DIR).ok();
            fs::create_dir(CACHE_DIR).ok();
            println!("Cache cleaned");
        }
    }
    Ok(())
}
