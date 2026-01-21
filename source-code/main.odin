package main

import "core:fmt"
import "core:os"
import "core:strings"
import "core:slice"
import "core:time"
import "core:math"
import "core:mem"
import "core:sort"
import "core:sys/unix" // For signal handling

// ANSI color codes for pretty CLI
ANSI_RESET      :: "\e[0m"
ANSI_BOLD       :: "\e[1m"
ANSI_RED        :: "\e[31m"
ANSI_GREEN      :: "\e[32m"
ANSI_YELLOW     :: "\e[33m"
ANSI_BLUE       :: "\e[34m"
ANSI_MAGENTA    :: "\e[35m"
ANSI_CYAN       :: "\e[36m"
ANSI_WHITE      :: "\e[37m"
ANSI_BG_RED     :: "\e[41m"
ANSI_BG_GREEN   :: "\e[42m"
ANSI_BG_YELLOW  :: "\e[43m"
ANSI_SAVE_POS   :: "\e[s"
ANSI_RESTORE_POS:: "\e[u"
ANSI_CURSOR_UP  :: "\e[1A"
ANSI_CLEAR_LINE :: "\e[2K"

// Foreign bindings to apt_wrapper (compiled from apt_wrapper.cpp)
foreign import apt_wrapper "apt_wrapper.o" // Assume linked

foreign apt_wrapper {
    apt_init_config :: proc "c" () ---
    apt_init_system :: proc "c" () ---
    apt_create_cachefile :: proc "c" () -> rawptr ---
    apt_open_cache :: proc "c" (cachefile: rawptr, with_lock: bool) ---
    apt_close_cache :: proc "c" (cachefile: rawptr) ---
    apt_delete_cachefile :: proc "c" (cachefile: rawptr) ---
    apt_get_depcache :: proc "c" (cachefile: rawptr) -> rawptr ---
    apt_pkg_begin :: proc "c" (cache: rawptr) -> rawptr ---
    apt_pkg_end :: proc "c" (iter: rawptr) -> bool ---
    apt_pkg_next :: proc "c" (iter: rawptr) ---
    apt_pkg_name :: proc "c" (iter: rawptr) -> cstring ---
    apt_pkg_fullname :: proc "c" (iter: rawptr, pretty: bool) -> cstring ---
    apt_pkg_candidate_ver :: proc "c" (iter: rawptr, cachefile: rawptr) -> rawptr ---
    apt_ver_section :: proc "c" (ver_iter: rawptr) -> cstring ---
    apt_ver_size :: proc "c" (ver_iter: rawptr) -> i64 ---
    apt_ver_installed_size :: proc "c" (ver_iter: rawptr) -> i64 ---
    apt_delete_iter :: proc "c" (iter: rawptr) ---
    apt_delete_ver_iter :: proc "c" (iter: rawptr) ---
    apt_mark_install :: proc "c" (depcache: rawptr, pkg_iter: rawptr, auto_inst: bool, from_user: bool) ---
    apt_mark_delete :: proc "c" (depcache: rawptr, pkg_iter: rawptr, purge: bool) ---
    apt_get_broken_count :: proc "c" (depcache: rawptr) -> i32 ---
    apt_get_del_count :: proc "c" (depcache: rawptr) -> i32 ---
    apt_get_inst_count :: proc "c" (depcache: rawptr) -> i32 ---
    apt_get_keep_count :: proc "c" (depcache: rawptr) -> i32 ---
    apt_get_download_size :: proc "c" (depcache: rawptr) -> i64 ---
    apt_get_disk_space_required :: proc "c" (depcache: rawptr) -> i64 ---
    apt_find_pkg :: proc "c" (cache: rawptr, name: cstring) -> rawptr ---
    apt_resolve :: proc "c" (depcache: rawptr, upgrade: bool) -> bool ---
    apt_update_cache :: proc "c" (progress: rawptr) -> bool --- // Note: progress not implemented yet
    apt_commit :: proc "c" (depcache: rawptr, progress: rawptr) -> bool ---
    apt_acquire_lock :: proc "c" () -> bool ---
    apt_release_lock :: proc "c" () ---
    apt_cleanup :: proc "c" () ---
}

// Struct for our state
Leg_State :: struct {
    cachefile: rawptr, // pkgCacheFile*
    depcache: rawptr,  // pkgDepCache*
    locked: bool,
}

// Global state for signal handling
global_state: ^Leg_State = nil

// Signal handler for SIGINT
sigint_handler :: proc(sig: i32) {
    if global_state != nil {
        cleanup_state(global_state)
    }
    os.exit(130) // Standard exit for SIGINT
}

// Initialize libapt
init_leg :: proc() -> Leg_State {
    state: Leg_State
    apt_init_config()
    apt_init_system()
    state.cachefile = apt_create_cachefile()
    state.locked = apt_acquire_lock()
    if !state.locked {
        log_error("Could not acquire lock. Is another package manager running?")
        os.exit(1)
    }
    apt_open_cache(state.cachefile, false) // Open without progress for now
    state.depcache = apt_get_depcache(state.cachefile)
    global_state = new(Leg_State)
    mem.copy(global_state, &state, size_of(Leg_State))
    // Set up signal handler
    unix.signal(unix.SIGINT, sigint_handler)
    return state
}

// Cleanup
cleanup_state :: proc(state: ^Leg_State) {
    if state.depcache != nil {
        // No need to delete depcache, owned by cachefile
    }
    if state.cachefile != nil {
        apt_close_cache(state.cachefile)
        apt_delete_cachefile(state.cachefile)
    }
    if state.locked {
        apt_release_lock()
        state.locked = false
    }
    apt_cleanup()
    free(global_state)
    global_state = nil
}

// Pretty log functions
log_info :: proc(msg: string) {
    fmt.printf("%s[INFO]%s %s\n", ANSI_CYAN, ANSI_RESET, msg)
}

log_warn :: proc(msg: string) {
    fmt.printf("%s[WARN]%s %s\n", ANSI_YELLOW, ANSI_RESET, msg)
}

log_error :: proc(msg: string) {
    fmt.printf("%s[ERROR]%s %s\n", ANSI_RED, ANSI_RESET, msg)
}

// Human-readable size
human_size :: proc(size: i64) -> string {
    units := []string{"B", "KB", "MB", "GB", "TB"}
    s := f64(size)
    i := 0
    for s >= 1024 && i < len(units)-1 {
        s /= 1024
        i += 1
    }
    return fmt.tprintf("%.2f %s", s, units[i])
}

// Get cache
get_cache :: proc(state: ^Leg_State) -> rawptr {
    return state.cachefile // Actually, GetPkgCache but in wrapper it's implicit
}

// Package info struct for grouping and display
PkgInfo :: struct {
    name: string,
    section: string,
    download_size: i64,
    installed_size: i64,
}

// Collect packages to install/update/remove
collect_changes :: proc(state: ^Leg_State, for_install: bool) -> (to_install: []PkgInfo, to_upgrade: []PkgInfo, to_remove: []PkgInfo, download_size: i64, disk_space: i64) {
    cache := get_cache(state)
    iter := apt_pkg_begin(cache)
    defer apt_delete_iter(iter)

    to_install = make([]PkgInfo, 0, 100)
    to_upgrade = make([]PkgInfo, 0, 100)
    to_remove = make([]PkgInfo, 0, 100)
    for !apt_pkg_end(iter) {
        ver := apt_pkg_candidate_ver(iter, state.cachefile)
        if ver != nil {
            defer apt_delete_ver_iter(ver)
            name := strings.clone_from_cstring(apt_pkg_name(iter))
            section := strings.clone_from_cstring(apt_ver_section(ver))
            dsize := apt_ver_size(ver)
            isize := apt_ver_installed_size(ver)
            // Note: To determine if install/upgrade/remove, we need to check state in depcache
            // For simplicity, assuming after marking, we can use counters, but for list, iterate and check flags
            // Actual check: pkgDepCache::StateCache &pkg = (*depcache)[*pkg_iter];
            // But wrapper doesn't have that yet; simulate or extend wrapper if needed
            // For now, placeholder: assume collect based on some logic
            // In real, extend wrapper to get install/upgrade state
        }
        apt_pkg_next(iter)
    }
    download_size = apt_get_download_size(state.depcache)
    disk_space = apt_get_disk_space_required(state.depcache)
    // Placeholder lists
    return
}

// Display preview table
display_preview :: proc(to_install, to_upgrade, to_remove: []PkgInfo, download_size, disk_space: i64) {
    fmt.println(ANSI_BOLD + "Packages to install:" + ANSI_RESET)
    for pkg in to_install {
        fmt.printf("%s %s (%s)\n", pkg.name, human_size(pkg.download_size), pkg.section)
    }
    fmt.println(ANSI_BOLD + "Packages to upgrade:" + ANSI_RESET)
    for pkg in to_upgrade {
        fmt.printf("%s %s (%s)\n", pkg.name, human_size(pkg.download_size), pkg.section)
    }
    fmt.println(ANSI_BOLD + "Packages to remove:" + ANSI_RESET)
    for pkg in to_remove {
        fmt.printf("%s\n", pkg.name)
    }
    fmt.printf("Download size: %s\n", human_size(download_size))
    fmt.printf("Disk space required: %s\n", human_size(disk_space))
}

// Ask confirmation
confirm_action :: proc() -> bool {
    fmt.print("Proceed? [Y/n] ")
    input: [1]byte
    os.read(os.stdin, input[:])
    return input[0] == 'Y' || input[0] == 'y' || input[0] == '\n'
}

// List all packages with table
list_packages :: proc(state: ^Leg_State) {
    cache := get_cache(state)
    iter := apt_pkg_begin(cache)
    defer apt_delete_iter(iter)

    // Collect all
    pkgs: [dynamic]PkgInfo
    defer delete(pkgs)
    for !apt_pkg_end(iter) {
        ver := apt_pkg_candidate_ver(iter, state.cachefile)
        if ver != nil {
            defer apt_delete_ver_iter(ver)
            append(&pkgs, PkgInfo{
                name = strings.clone_from_cstring(apt_pkg_fullname(iter, true)),
                section = strings.clone_from_cstring(apt_ver_section(ver)),
            })
        }
        apt_pkg_next(iter)
    }

    // Sort by name
    sort.quick_sort_proc(pkgs[:], proc(a, b: PkgInfo) -> int {
        return strings.compare(a.name, b.name)
    })

    // Table header
    fmt.printf("%-40s %-20s\n", ANSI_BOLD + "Package" + ANSI_RESET, ANSI_BOLD + "Section" + ANSI_RESET)
    fmt.println(strings.repeat("-", 60))

    for pkg in pkgs {
        fmt.printf("%s%-40s%s %s%-20s%s\n", ANSI_GREEN, pkg.name, ANSI_RESET, ANSI_BLUE, pkg.section, ANSI_RESET)
    }
}

// Group packages by section for update
group_packages :: proc(pkgs: []PkgInfo) -> map[string][]string {
    groups: map[string][]string
    for pkg in pkgs {
        if pkg.section not_in groups {
            groups[pkg.section] = make([]string, 0, 10)
        }
        append(&groups[pkg.section], pkg.name)
    }
    return groups
}

// Update command with grouping
update :: proc(state: ^Leg_State) {
    log_info("Updating repositories")
    apt_update_cache(nil) // Progress nil for now
    log_info("Update complete")
    // For upgrade, mark all for upgrade
    cache := get_cache(state)
    iter := apt_pkg_begin(cache)
    defer apt_delete_iter(iter)
    for !apt_pkg_end(iter) {
        if apt_pkg_candidate_ver(iter, state.cachefile) != nil {
            apt_mark_install(state.depcache, iter, true, false)
        }
        apt_pkg_next(iter)
    }
    apt_resolve(state.depcache, true)
    to_install, to_upgrade, to_remove, dsize, dspace := collect_changes(state, false)
    defer {
        delete(to_install); delete(to_upgrade); delete(to_remove)
    }
    // Display grouped
    groups := group_packages(to_upgrade)
    defer {
        for k, v in groups { delete(v) }
        delete(groups)
    }
    for section, names in groups {
        fmt.printf("%s%s:%s\n", ANSI_MAGENTA, section, ANSI_RESET)
        for name in names {
            fmt.printf("  %s%s%s\n", ANSI_YELLOW, name, ANSI_RESET)
        }
    }
    display_preview(to_install, to_upgrade, to_remove, dsize, dspace)
    if confirm_action() {
        apt_commit(state.depcache, nil)
    }
}

// Progress bar
progress_bar :: proc(width: int, percent: f32, label: string, color: string) {
    filled := int(percent / 100 * f32(width))
    bar := strings.repeat("█", filled)
    empty := strings.repeat(" ", width - filled)
    fmt.printf("%s%s: %s%s%s%s %3.0f%%%s", ANSI_BOLD, label, color, bar, ANSI_RESET, empty, percent, ANSI_RESET)
}

// Simulate parallel downloads with ANSI cursor control
simulate_parallel_downloads :: proc(labels: []string, durations: []int) {
    num := len(labels)
    starts: [dynamic]time.Time
    percents: [dynamic]f32
    for _ in 0..<num {
        append(&starts, time.now())
        append(&percents, 0.0)
    }
    defer { delete(starts); delete(percents) }

    // Print initial bars
    for i in 0..<num {
        progress_bar(50, 0, labels[i], ANSI_BG_YELLOW)
        fmt.println()
    }

    for {
        all_done := true
        fmt.print(ANSI_SAVE_POS)
        for i in 0..<num {
            fmt.print(ANSI_CURSOR_UP)
            fmt.print(ANSI_CLEAR_LINE)
            elapsed := time.diff(starts[i], time.now())
            percent := f32(time.duration_milliseconds(elapsed) / f32(durations[i]) * 100)
            if percent > 100 { percent = 100 }
            percents[i] = percent
            color := ANSI_BG_GREEN if percent > 50 else ANSI_BG_YELLOW if percent > 20 else ANSI_BG_RED
            progress_bar(50, percent, labels[i], color)
            if percent < 100 { all_done = false }
        }
        fmt.print(ANSI_RESTORE_POS)
        os.flush(os.stdout)
        if all_done { break }
        time.sleep(100 * time.Millisecond)
    }
    fmt.println()
}

// Resolve dependencies for a package
resolve_deps :: proc(state: ^Leg_State, pkg_name: string, for_install: bool) -> bool {
    pkg_iter := apt_find_pkg(get_cache(state), strings.clone_to_cstring(pkg_name))
    if pkg_iter == nil {
        log_error("Package not found: " + pkg_name)
        return false
    }
    defer apt_delete_iter(pkg_iter)

    if for_install {
        apt_mark_install(state.depcache, pkg_iter, true, true)
    } else {
        apt_mark_delete(state.depcache, pkg_iter, false)
    }
    if !apt_resolve(state.depcache, false) {
        log_warn("Failed to resolve dependencies")
        return false
    }
    return apt_get_broken_count(state.depcache) == 0
}

// Install command
install :: proc(state: ^Leg_State, pkg_name: string) {
    log_info("Preparing to install " + pkg_name)
    if resolve_deps(state, pkg_name, true) {
        to_install, to_upgrade, to_remove, dsize, dspace := collect_changes(state, true)
        defer { delete(to_install); delete(to_upgrade); delete(to_remove) }
        display_preview(to_install, to_upgrade, to_remove, dsize, dspace)
        if confirm_action() {
            // Simulate parallel if multiple
            if len(to_install) > 1 {
                labels := make([]string, len(to_install))
                durations := make([]int, len(to_install))
                for i, pkg in to_install {
                    labels[i] = "Downloading " + pkg.name
                    durations[i] = 2000 + i*500 // Vary
                }
                simulate_parallel_downloads(labels, durations)
                delete(labels)
                delete(durations)
            } else {
                simulate_task("Downloading", 2000)
            }
            simulate_task("Installing", 3000)
            if apt_commit(state.depcache, nil) {
                log_info("Installed successfully")
            } else {
                log_error("Installation failed")
            }
        } else {
            log_info("Installation aborted")
        }
    } else {
        log_error("Dependency resolution failed")
    }
}

// Remove command
remove :: proc(state: ^Leg_State, pkg_name: string) {
    log_info("Preparing to remove " + pkg_name)
    if resolve_deps(state, pkg_name, false) {
        to_install, to_upgrade, to_remove, dsize, dspace := collect_changes(state, false)
        defer { delete(to_install); delete(to_upgrade); delete(to_remove) }
        display_preview(to_install, to_upgrade, to_remove, dsize, dspace)
        if confirm_action() {
            simulate_task("Removing", 1500)
            if apt_commit(state.depcache, nil) {
                log_info("Removed successfully")
            } else {
                log_error("Removal failed")
            }
        } else {
            log_info("Removal aborted")
        }
    } else {
        log_error("Dependency resolution failed")
    }
}

// Simulated task with progress (fallback for single)
simulate_task :: proc(label: string, duration_ms: int) {
    start := time.now()
    for {
        elapsed := time.diff(start, time.now())
        percent := f32(time.duration_milliseconds(elapsed) / f32(duration_ms) * 100)
        if percent > 100 { percent = 100 }
        fmt.print("\r")
        progress_bar(50, percent, label, ANSI_BG_GREEN if percent > 50 else ANSI_BG_YELLOW if percent > 20 else ANSI_BG_RED)
        os.flush(os.stdout)
        if percent >= 100 { break }
        time.sleep(100 * time.Millisecond)
    }
    fmt.println()
}

// Main CLI parser
main :: proc() {
    if len(os.args) < 2 {
        fmt.println("Usage: legendarny [install|remove|update|list] [pkg]")
        return
    }

    state := init_leg()
    defer cleanup_state(&state)

    cmd := os.args[1]
    switch cmd {
    case "install":
        if len(os.args) < 3 { log_error("Missing package name"); return }
        install(&state, os.args[2])
    case "remove":
        if len(os.args) < 3 { log_error("Missing package name"); return }
        remove(&state, os.args[2])
    case "update":
        update(&state)
    case "list":
        list_packages(&state)
    case:
        log_error("Unknown command: " + cmd)
    }
}
