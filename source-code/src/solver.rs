use anyhow::{anyhow, Result};
use std::ffi::{CString, CStr};
use std::ptr;
use crate::repo::PackageMetadata;
use crate::db::Database;
use crate::ffi;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Transaction {
    pub install: Vec<PackageMetadata>,
    pub remove: Vec<PackageMetadata>,
    pub upgrade: Vec<PackageMetadata>,
    pub reinstall: Vec<PackageMetadata>,
    pub total_download: u64,
}

pub async fn resolve(pkgs: &[String]) -> Result<Transaction> {
    unsafe {
        let pool = ffi::pool_create();
        // Force Architecture to amd64 for solving context
        let arch = CString::new("amd64").unwrap();
        ffi::pool_setarch(pool, arch.as_ptr());

        // 1. Initialize System Repo (Installed Packages)
        let sys_repo_name = CString::new("system").unwrap();
        let sys_repo = ffi::repo_create(pool, sys_repo_name.as_ptr());
        let sys_data = ffi::repo_add_repodata(sys_repo, 0);

        let db = Database::load();
        for (_, pkg) in db.packages {
            let id = ffi::repo_add_solvable(sys_repo);
            let name = CString::new(pkg.name).unwrap();
            let ver = CString::new(pkg.version).unwrap();
            // Critical Fix: Installed packages MUST have an architecture, otherwise solver crashes
            let pkg_arch_str = if pkg.arch.is_empty() { "amd64" } else { &pkg.arch };
            let pkg_arch = CString::new(pkg_arch_str).unwrap();

            ffi::repodata_set_str(sys_data, id, ffi::SOLVABLE_NAME, name.as_ptr());
            ffi::repodata_set_str(sys_data, id, ffi::SOLVABLE_EVR, ver.as_ptr());
            ffi::repodata_set_str(sys_data, id, ffi::SOLVABLE_ARCH, pkg_arch.as_ptr());
        }
        ffi::repodata_internalize(sys_data);
        ffi::repo_internalize(sys_repo);

        // Mark system repo as "installed"
        ffi::pool_set_installed(pool, sys_repo);

        // 2. Initialize Available Repos from Cache
        let list_dir = std::path::Path::new("/var/lib/lpm/lists");
        if list_dir.exists() {
            for entry in std::fs::read_dir(list_dir)? {
                let entry = entry?;
                let path = entry.path();
                // Ensure we only process files that are actually package lists
                if path.to_string_lossy().contains("_Packages") && path.is_file() {
                    let repo_name = CString::new(path.file_name().unwrap().to_str().unwrap()).unwrap();
                    let repo = ffi::repo_create(pool, repo_name.as_ptr());

                    // Libsolv expects a file pointer. We open the text file.
                    let c_path = CString::new(path.to_str().unwrap()).unwrap();
                    let mode = CString::new("r").unwrap();
                    let fp = libc::fopen(c_path.as_ptr(), mode.as_ptr());

                    if !fp.is_null() {
                        // Standard Debian parsing
                        ffi::repo_add_debpackages(repo, fp, 0);
                        libc::fclose(fp);
                    }
                    ffi::repo_internalize(repo);
                }
            }
        }

        // 3. Prepare Pool for Solving
        // CRITICAL: Build the "what provides" index. Without this, solving by name causes segfaults.
        ffi::pool_createwhatprovides(pool);

        // 4. Create Job Queue
        // Initialize with null to let C side or explicit initialization handle it if needed,
        // but ffi::queue_init usually does the malloc.
        let mut queue = ffi::Queue { elements: ptr::null_mut(), count: 0, alloc: 0, left: 0 };
        ffi::queue_init(&mut queue);

        for pkg_str in pkgs {
            // Determine action and add Flag to indicate we are providing a NAME ID, not a Solvable ID
            let (name_str, action) = if pkg_str.ends_with('-') {
                (&pkg_str[..pkg_str.len()-1], ffi::SOLVER_ERASE | ffi::SOLVER_SOLVABLE_NAME)
            } else {
                (pkg_str.as_str(), ffi::SOLVER_INSTALL | ffi::SOLVER_SOLVABLE_NAME)
            };

            let c_str = CString::new(name_str).unwrap();
            // We ask for the ID. If it doesn't exist, it returns 0.
            let id = ffi::pool_str2id(pool, c_str.as_ptr(), 0);

            if id == 0 {
                if (action & ffi::SOLVER_INSTALL) != 0 {
                    eprintln!("Warning: Package '{}' not found in any repository.", name_str);
                }
                // If removing a package that doesn't exist, we just skip it (it's already 'removed')
            } else {
                ffi::queue_insert(&mut queue, queue.count, action);
                ffi::queue_insert(&mut queue, queue.count, id);
            }
        }

        // 5. Solve Dependencies
        let solver = ffi::solver_create(pool);
        let problem_count = ffi::solver_solve(solver, &mut queue);

        if problem_count != 0 {
            ffi::queue_free(&mut queue);
            ffi::solver_free(solver);
            ffi::pool_free(pool);
            return Err(anyhow!("Solver found {} problems. (Conflicts or missing dependencies)", problem_count));
        }

        // 6. Generate Transaction
        let trans_ptr = ffi::solver_create_transaction(solver);
        let mut decision_q = ffi::Queue { elements: ptr::null_mut(), count: 0, alloc: 0, left: 0 };
        ffi::queue_init(&mut decision_q);
        ffi::transaction_create_decisionq(trans_ptr, &mut decision_q, ptr::null_mut());

        let mut tx = Transaction {
            install: vec![],
            remove: vec![],
            upgrade: vec![],
            reinstall: vec![],
            total_download: 0
        };

        if decision_q.count > 0 && !decision_q.elements.is_null() {
            let elements = std::slice::from_raw_parts(decision_q.elements, decision_q.count as usize);

            for &solvid in elements {
                if solvid <= 0 { continue; }

                let type_ = ffi::transaction_type(trans_ptr, solvid, ffi::SOLVER_TRANSACTION_SHOW_ALL);
                let pkg_meta = extract_metadata(pool, solvid);

                match type_ {
                    ffi::SOLVER_TRANSACTION_INSTALL => {
                        tx.total_download += pkg_meta.size;
                        tx.install.push(pkg_meta);
                    },
                    ffi::SOLVER_TRANSACTION_ERASE => {
                        tx.remove.push(pkg_meta);
                    },
                    ffi::SOLVER_TRANSACTION_UPGRADE => {
                        tx.total_download += pkg_meta.size;
                        tx.upgrade.push(pkg_meta);
                    },
                    ffi::SOLVER_TRANSACTION_DOWNGRADE => {
                        tx.total_download += pkg_meta.size;
                        tx.install.push(pkg_meta);
                    },
                    _ => {}
                }
            }
        }

        ffi::queue_free(&mut decision_q);
        ffi::transaction_free(trans_ptr);
        ffi::queue_free(&mut queue);
        ffi::solver_free(solver);
        ffi::pool_free(pool);

        Ok(tx)
    }
}

unsafe fn extract_metadata(pool: *mut ffi::Pool, solvid: libc::c_int) -> PackageMetadata {
    let get_str = |key| {
        let ptr = ffi::pool_lookup_str(pool, solvid, key);
        if ptr.is_null() { String::new() }
        else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
    };

    let get_num = |key| {
        ffi::pool_lookup_num(pool, solvid, key, 0)
    };

    PackageMetadata {
        name: get_str(ffi::SOLVABLE_NAME),
        version: get_str(ffi::SOLVABLE_EVR),
        description: get_str(ffi::SOLVABLE_DESCRIPTION),
        size: get_num(ffi::SOLVABLE_DOWNLOADSIZE),
        architecture: get_str(ffi::SOLVABLE_ARCH),
        filename: String::new(),
        ..Default::default()
    }
}
