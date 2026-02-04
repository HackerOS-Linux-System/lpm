use libc::{c_char, c_int, c_void, FILE};

#[repr(C)]
pub struct Pool { _private: [u8; 0] }
#[repr(C)]
pub struct Repo { _private: [u8; 0] }
#[repr(C)]
pub struct Repodata { _private: [u8; 0] }
#[repr(C)]
pub struct Solver { _private: [u8; 0] }
#[repr(C)]
pub struct Transaction { _private: [u8; 0] }
#[repr(C)]
pub struct Queue {
    pub elements: *mut c_int,
    pub count: c_int,
    pub alloc: c_int,
    pub left: c_int,
}

// Modern Libsolv Constants (0.7.x+)
pub const SOLVER_INSTALL: c_int = 0xc000;
pub const SOLVER_ERASE: c_int = 0xc100;
pub const SOLVER_UPDATE: c_int = 0xc200;

pub const SOLVER_SOLVABLE: c_int = 0x00;
pub const SOLVER_SOLVABLE_NAME: c_int = 0x01;
pub const SOLVER_SOLVABLE_PROVIDES: c_int = 0x02;

// Transaction Constants
pub const SOLVER_TRANSACTION_SHOW_ALL: c_int = 1 << 4;
pub const SOLVER_TRANSACTION_INSTALL: c_int = 11;
pub const SOLVER_TRANSACTION_ERASE: c_int = 12;
pub const SOLVER_TRANSACTION_UPGRADE: c_int = 13;
pub const SOLVER_TRANSACTION_DOWNGRADE: c_int = 14;

// Keys
pub const SOLVABLE_NAME: c_int = 1;
pub const SOLVABLE_ARCH: c_int = 2;
pub const SOLVABLE_EVR: c_int = 3;
pub const SOLVABLE_DESCRIPTION: c_int = 14;
pub const SOLVABLE_DOWNLOADSIZE: c_int = 23;
pub const SOLVABLE_CHECKSUM: c_int = 27;

extern "C" {
    // Pool
    pub fn pool_create() -> *mut Pool;
    pub fn pool_free(pool: *mut Pool);
    pub fn pool_setarch(pool: *mut Pool, arch: *const c_char);
    pub fn pool_str2id(pool: *mut Pool, str: *const c_char, create: c_int) -> c_int;
    pub fn pool_set_installed(pool: *mut Pool, repo: *mut Repo);
    pub fn pool_createwhatprovides(pool: *mut Pool); // Critical for Name resolution

    // Lookups
    pub fn pool_id2str(pool: *mut Pool, id: c_int) -> *const c_char;
    pub fn pool_lookup_str(pool: *mut Pool, solvid: c_int, key: c_int) -> *const c_char;
    pub fn pool_lookup_num(pool: *mut Pool, solvid: c_int, key: c_int, notfound: u64) -> u64;
    pub fn pool_lookup_checksum(pool: *mut Pool, solvid: c_int, key: c_int, type_id: *mut c_int) -> *const c_char;

    // Repo
    pub fn repo_create(pool: *mut Pool, name: *const c_char) -> *mut Repo;
    pub fn repo_add_debpackages(repo: *mut Repo, fp: *mut FILE, flags: c_int) -> c_int;
    pub fn repo_internalize(repo: *mut Repo);
    pub fn repo_add_solvable(repo: *mut Repo) -> c_int;
    pub fn repo_add_repodata(repo: *mut Repo, flags: c_int) -> *mut Repodata;

    // Repodata
    pub fn repodata_internalize(data: *mut Repodata);
    pub fn repodata_set_str(data: *mut Repodata, solvid: c_int, key: c_int, str: *const c_char);
    pub fn repodata_set_num(data: *mut Repodata, solvid: c_int, key: c_int, num: u64);

    // Solver
    pub fn solver_create(pool: *mut Pool) -> *mut Solver;
    pub fn solver_solve(solver: *mut Solver, job: *mut Queue) -> c_int;
    pub fn solver_create_transaction(solver: *mut Solver) -> *mut Transaction;
    pub fn solver_free(solver: *mut Solver);

    // Transaction
    pub fn transaction_create_decisionq(trans: *mut Transaction, decisionq: *mut Queue, weakness: *mut c_void);
    pub fn transaction_type(trans: *mut Transaction, solvid: c_int, mode: c_int) -> c_int;
    pub fn transaction_free(trans: *mut Transaction);

    // Queue
    pub fn queue_init(q: *mut Queue);
    pub fn queue_insert(q: *mut Queue, pos: c_int, id: c_int);
    pub fn queue_free(q: *mut Queue);
}
