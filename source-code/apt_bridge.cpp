#include "legendary/src/apt_bridge.h"
#include <apt-pkg/init.h>
#include <apt-pkg/error.h>
#include <apt-pkg/cachefile.h>
#include <apt-pkg/pkgcache.h>
#include <apt-pkg/depcache.h>
#include <apt-pkg/cacheiterators.h>
#include <apt-pkg/algorithms.h>
#include <apt-pkg/update.h>
#include <apt-pkg/acquire-item.h>
#include <apt-pkg/sourcelist.h>
#include <apt-pkg/pkgsystem.h>
#include <apt-pkg/upgrade.h>
#include <apt-pkg/configuration.h>
#include <iostream>

// Ensure global variables from libapt-pkg are visible
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

rust::Vec<PkgInfo> AptClient::list_all() {
    rust::Vec<PkgInfo> result;

    if (impl->CacheFile.GetPkgCache() == nullptr) return result;

    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->PkgBegin();
    for (; !P.end(); ++P) {
        // Use the iterator method to check for versions
        if (P.VersionList().end()) continue;

        PkgInfo info;
        info.name = P.Name();

        pkgCache::VerIterator V = P.VersionList();

        if (!V.end() && V.Section()) {
            info.section = V.Section();
        } else {
            info.section = "unknown";
        }

        info.version = V.VerStr();
        info.size = V->Size;
        info.current_state = (!P.CurrentVer().end()) ? 1 : 0;

        result.push_back(info);
    }
    return result;
}

PkgInfo AptClient::find_package(rust::String name) {
    PkgInfo info;
    info.name = "";

    if (impl->CacheFile.GetPkgCache() == nullptr) return info;

    std::string s_name = std::string(name);
    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);

    if (P.end()) {
        return info;
    }

    info.name = P.Name();

    if (impl->CacheFile.GetDepCache() != nullptr) {
        pkgDepCache::StateCache &State = (*impl->CacheFile.GetDepCache())[P];

        // CandidateVer is a raw pointer (Version *) in StateCache
        if (State.CandidateVer != nullptr) {
            pkgCache::VerIterator V = State.CandidateVerIter(*impl->CacheFile.GetDepCache());
            info.version = V.VerStr();
            info.size = V->Size;
            if (V.Section()) {
                info.section = V.Section();
            } else {
                info.section = "unknown";
            }
        } else if (!P.CurrentVer().end()) {
            pkgCache::VerIterator V = P.CurrentVer();
            info.version = V.VerStr();
            info.size = V->Size;
            if (V.Section()) {
                info.section = V.Section();
            } else {
                info.section = "installed";
            }
        } else {
            info.version = "N/A";
            info.section = "N/A";
            info.size = 0;
        }
    } else {
        if (!P.CurrentVer().end()) {
            pkgCache::VerIterator V = P.CurrentVer();
            info.version = V.VerStr();
        } else {
            info.version = "N/A";
        }
        info.section = "unknown";
        info.size = 0;
    }

    info.current_state = (!P.CurrentVer().end()) ? 1 : 0;
    return info;
}

void AptClient::mark_install(rust::String name) {
    std::string s_name = std::string(name);
    if (impl->CacheFile.GetPkgCache() == nullptr) return;

    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (!P.end()) {
        impl->CacheFile.GetDepCache()->MarkInstall(P, true);
    }
}

void AptClient::mark_remove(rust::String name) {
    std::string s_name = std::string(name);
    if (impl->CacheFile.GetPkgCache() == nullptr) return;

    pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->FindPkg(s_name);
    if (!P.end()) {
        impl->CacheFile.GetDepCache()->MarkDelete(P, false);
    }
}

void AptClient::mark_upgrade() {
    if (impl->CacheFile.GetDepCache() == nullptr) return;
    // Use APT::Upgrade::Upgrade which is the modern C++ equivalent of the algorithm wrappers
    // Mode 0 = ALLOW_EVERYTHING (Dist-Upgrade equivalent)
    APT::Upgrade::Upgrade(*impl->CacheFile.GetDepCache(), 0);
}

bool AptClient::resolve() {
    if (impl->CacheFile.GetDepCache() == nullptr) return false;
    return pkgFixBroken(*impl->CacheFile.GetDepCache());
}

int64_t AptClient::get_download_size() const {
    if (impl->CacheFile.GetDepCache() == nullptr) return 0;
    return impl->CacheFile.GetDepCache()->DebSize();
}

bool AptClient::commit_changes() {
    return true;
}

std::unique_ptr<AptClient> new_apt_client() {
    return std::make_unique<AptClient>();
}
