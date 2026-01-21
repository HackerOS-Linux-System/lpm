#include <apt-pkg/cachefile.h>
#include <apt-pkg/pkgcache.h>
#include <apt-pkg/policy.h>
#include <apt-pkg/acquire.h>
#include <apt-pkg/acquire-item.h>
#include <apt-pkg/progress.h>
#include <apt-pkg/init.h>
#include <apt-pkg/error.h>
#include <apt-pkg/sptr.h>
#include <apt-pkg/depcache.h>
#include <apt-pkg/configuration.h>
#include <apt-pkg/pkgrecords.h>
#include <apt-pkg/algorithms.h>
#include <apt-pkg/update.h>
#include <apt-pkg/install-progress.h>
#include <apt-pkg/cmndline.h>
#include <signal.h>
#include <unistd.h>
#include <iostream>
#include <vector>
#include <string>

extern "C" {

// Initialization
void apt_init_config() {
    pkgInitConfig(*_config);
}

void apt_init_system() {
    pkgInitSystem(*_config, _system);
}

// pkgCacheFile management
void* apt_create_cachefile() {
    return new pkgCacheFile();
}

void apt_open_cache(void* cachefile, bool with_lock) {
    static_cast<pkgCacheFile*>(cachefile)->Open(NULL, with_lock);
}

void apt_close_cache(void* cachefile) {
    static_cast<pkgCacheFile*>(cachefile)->Close();
}

void apt_delete_cachefile(void* cachefile) {
    delete static_cast<pkgCacheFile*>(cachefile);
}

// pkgDepCache access
void* apt_get_depcache(void* cachefile) {
    return static_cast<pkgCacheFile*>(cachefile)->GetDepCache();
}

// Package iterator
void* apt_pkg_begin(void* cache) {
    pkgCache* c = static_cast<pkgCache*>(cache);
    return new pkgCache::PkgIterator(c->PkgBegin());
}

bool apt_pkg_end(void* iter) {
    return static_cast<pkgCache::PkgIterator*>(iter)->end();
}

void apt_pkg_next(void* iter) {
    (*(static_cast<pkgCache::PkgIterator*>(iter)))++;
}

const char* apt_pkg_name(void* iter) {
    return static_cast<pkgCache::PkgIterator*>(iter)->Name();
}

const char* apt_pkg_fullname(void* iter, bool pretty) {
    return static_cast<pkgCache::PkgIterator*>(iter)->FullName(pretty).c_str();
}

void* apt_pkg_candidate_ver(void* iter, void* cachefile) {
    pkgCache::PkgIterator* pkg = static_cast<pkgCache::PkgIterator*>(iter);
    pkgPolicy* policy = static_cast<pkgCacheFile*>(cachefile)->GetPolicy();
    pkgCache::VerIterator ver = policy->GetCandidateVer(*pkg);
    if (ver.end()) return nullptr;
    return new pkgCache::VerIterator(ver);
}

const char* apt_ver_section(void* ver_iter) {
    return static_cast<pkgCache::VerIterator*>(ver_iter)->Section();
}

long apt_ver_size(void* ver_iter) {
    return static_cast<pkgCache::VerIterator*>(ver_iter)->Size();
}

long apt_ver_installed_size(void* ver_iter) {
    return static_cast<pkgCache::VerIterator*>(ver_iter)->InstalledSize();
}

void apt_delete_iter(void* iter) {
    delete static_cast<pkgCache::PkgIterator*>(iter);
}

void apt_delete_ver_iter(void* iter) {
    delete static_cast<pkgCache::VerIterator*>(iter);
}

// Dependency resolution
void apt_mark_install(void* depcache, void* pkg_iter, bool auto_inst, bool from_user) {
    pkgDepCache* dc = static_cast<pkgDepCache*>(depcache);
    dc->MarkInstall(*(static_cast<pkgCache::PkgIterator*>(pkg_iter)), auto_inst, 0, from_user);
}

void apt_mark_delete(void* depcache, void* pkg_iter, bool purge) {
    pkgDepCache* dc = static_cast<pkgDepCache*>(depcache);
    dc->MarkDelete(*(static_cast<pkgCache::PkgIterator*>(pkg_iter)), purge);
}

int apt_get_broken_count(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->BrokenCount();
}

int apt_get_del_count(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->DelCount();
}

int apt_get_inst_count(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->InstCount();
}

int apt_get_keep_count(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->KeepCount();
}

long apt_get_download_size(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->DebSize();
}

long apt_get_disk_space_required(void* depcache) {
    return static_cast<pkgDepCache*>(depcache)->UsrSize();
}

// Find package by name
void* apt_find_pkg(void* cache, const char* name) {
    pkgCache* c = static_cast<pkgCache*>(cache);
    pkgCache::PkgIterator* iter = new pkgCache::PkgIterator(c->FindPkg(name));
    if (iter->end()) {
        delete iter;
        return nullptr;
    }
    return iter;
}

// Resolve dependencies
bool apt_resolve(void* depcache, bool upgrade) {
    return pkgResolveDeps(static_cast<pkgDepCache*>(depcache), upgrade);
}

// Update repositories
bool apt_update_cache(OpProgress* progress) {
    pkgAcquire fetcher;
    return ListUpdate(fetcher, *pkgSourceList::GetList(), progress);
}

// Commit changes
bool apt_commit(void* depcache, OpProgress* progress) {
    pkgPackageManager* pm = _system->CreatePM(static_cast<pkgDepCache*>(depcache));
    if (!pm->GetArchives(static_cast<pkgDepCache*>(depcache), pkgSourceList::GetList(), new pkgRecords(*(static_cast<pkgDepCache*>(depcache))))) {
        delete pm;
        return false;
    }
    pkgPackageManager::OrderResult res = pm->DoInstall(progress);
    delete pm;
    return res == pkgPackageManager::Completed;
}

// Lock management
bool apt_acquire_lock() {
    return _system->Lock();
}

void apt_release_lock() {
    _system->UnLock();
}

// Signal handling placeholder (handled in Odin, but can add cleanup hook if needed)
void apt_cleanup() {
    // Any global cleanup if needed
    _error->Discard();
}

}
