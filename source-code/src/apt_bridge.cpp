#include "legendary/src/apt_bridge.h"
#include <apt-pkg/init.h>
#include <apt-pkg/error.h>
#include <apt-pkg/cachefile.h>
#include <apt-pkg/pkgcache.h>
#include <apt-pkg/depcache.h>
#include <apt-pkg/cacheiterators.h>
#include <apt-pkg/algorithms.h>
#include <apt-pkg/update.h>
#include <apt-pkg/clean.h>
#include <apt-pkg/upgrade.h>
#include <apt-pkg/configuration.h>
#include <apt-pkg/pkgrecords.h>
#include <apt-pkg/acquire.h>
#include <iostream>
#include <sstream>

extern pkgSystem *_system;
extern Configuration *_config;

class AptClient::Impl {
public:
    pkgCacheFile CacheFile;

    Impl() {}

    void Init() {
        if (_config == nullptr || _system == nullptr) {
            pkgInitConfig(*_config);
            pkgInitSystem(*_config, _system);
        }
    }

    void Open() {
        if (CacheFile.Open(nullptr, false) == false) {
            _error->DumpErrors();
        }
    }
};

AptClient::AptClient() : impl(new Impl()) {}
AptClient::~AptClient() {}

void AptClient::init() {
    impl->Init();
    impl->Open();
}

void AptClient::update_cache() {
    if (impl->CacheFile.BuildCaches(nullptr, true) == false) {
        _error->DumpErrors();
    }
}

// Helper to fill PkgInfo
PkgInfo fill_info(pkgCache::PkgIterator P, pkgCacheFile &CacheFile) {
    PkgInfo info;
    info.name = P.Name();
    info.current_state = (!P.CurrentVer().end()) ? 1 : 0;

    pkgCache::VerIterator V;
    if (CacheFile.GetDepCache() != nullptr) {
        pkgDepCache::StateCache &State = (*CacheFile.GetDepCache())[P];
        if (State.CandidateVer != nullptr) {
            V = State.CandidateVerIter(*CacheFile.GetDepCache());
        } else if (!P.CurrentVer().end()) {
            V = P.CurrentVer();
        }
    } else if (!P.CurrentVer().end()) {
        V = P.CurrentVer();
    }

    if (!V.end()) {
        info.version = V.VerStr();
        info.size = V->Size;
        info.section = V.Section() ? V.Section() : "unknown";
    } else {
        info.version = "N/A";
        info.size = 0;
        info.section = "unknown";
    }
    return info;
}

rust::Vec<PkgInfo> AptClient::list_all() {
    rust::Vec<PkgInfo> result;
    if (impl->CacheFile.GetPkgCache() == nullptr) return result;

    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->PkgBegin();
    for (; !P.end(); ++P) {
        if (P.VersionList().end()) continue;
        result.push_back(fill_info(P, impl->CacheFile));
    }
    return result;
}

rust::Vec<PkgInfo> AptClient::search(rust::String term) {
    rust::Vec<PkgInfo> result;
    if (impl->CacheFile.GetPkgCache() == nullptr) return result;

    std::string q = std::string(term);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->PkgBegin();

    for (; !P.end(); ++P) {
        if (P.VersionList().end()) continue;

        std::string name = P.Name();
        bool match = name.find(q) != std::string::npos;

        // Search description if name doesn't match
        if (!match) {
            pkgCache::VerIterator V = P.VersionList(); // Simplification: check first version
            if (!V.end()) {
                pkgCache::DescIterator D = V.DescriptionList();
                if (!D.end()) {
                    pkgRecords Recs(impl->CacheFile);
                    pkgRecords::Parser &Parser = Recs.Lookup(D.FileList());
                    std::string desc = Parser.ShortDesc();
                    if (desc.find(q) != std::string::npos) match = true;
                }
            }
        }

        if (match) {
            result.push_back(fill_info(P, impl->CacheFile));
        }
    }
    return result;
}

PkgInfo AptClient::find_package(rust::String name) {
    if (impl->CacheFile.GetPkgCache() == nullptr) return PkgInfo{};
    std::string s_name = std::string(name);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (P.end()) return PkgInfo{};
    return fill_info(P, impl->CacheFile);
}

PkgDetails AptClient::get_package_details(rust::String name) {
    PkgDetails details;
    details.name = std::string(name);

    if (impl->CacheFile.GetPkgCache() == nullptr) return details;
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(std::string(name));
    if (P.end()) return details;

    pkgCache::VerIterator V;
    if (impl->CacheFile.GetDepCache()) {
        pkgDepCache::StateCache &State = (*impl->CacheFile.GetDepCache())[P];
        V = State.CandidateVerIter(*impl->CacheFile.GetDepCache());
    }
    if (V.end()) V = P.CurrentVer();
    if (V.end()) return details;

    details.version = V.VerStr();
    details.section = V.Section() ? V.Section() : "";
    details.maintainer = "Unknown"; // Requires parsing binary data usually
    details.installed_size = V->InstalledSize;
    details.download_size = V->Size;

    pkgRecords Recs(impl->CacheFile);
    pkgRecords::Parser &Parser = Recs.Lookup(V.DescriptionList().FileList());
    details.description = Parser.LongDesc();

    // Dependencies
    for (pkgCache::DepIterator D = V.DependsList(); !D.end(); ++D) {
        if (D->Type == pkgCache::Dep::Depends) {
            details.dependencies.push_back(D.TargetPkg().Name());
        }
    }

    return details;
}

void AptClient::mark_install(rust::String name) {
    std::string s_name = std::string(name);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (!P.end()) impl->CacheFile.GetDepCache()->MarkInstall(P, true);
}

void AptClient::mark_remove(rust::String name) {
    std::string s_name = std::string(name);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (!P.end()) impl->CacheFile.GetDepCache()->MarkDelete(P, false);
}

void AptClient::mark_auto(rust::String name, bool is_auto) {
    std::string s_name = std::string(name);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (!P.end()) impl->CacheFile.GetDepCache()->MarkAuto(P, is_auto);
}

void AptClient::mark_upgrade() {
    if (impl->CacheFile.GetDepCache())
        APT::Upgrade::Upgrade(*impl->CacheFile.GetDepCache(), 0);
}

bool AptClient::resolve() {
    return impl->CacheFile.GetDepCache() ? pkgFixBroken(*impl->CacheFile.GetDepCache()) : false;
}

int64_t AptClient::get_download_size() const {
    return impl->CacheFile.GetDepCache() ? impl->CacheFile.GetDepCache()->DebSize() : 0;
}

bool AptClient::commit_changes() {
    // In a real scenario, this involves Acquire and PackageManager
    // For this simulation/bridge:
    return true;
}

void AptClient::clean_cache() {
    pkgAcquire Fetcher;
    Fetcher.Clean("");
}

std::unique_ptr<AptClient> new_apt_client() {
    return std::make_unique<AptClient>();
}
