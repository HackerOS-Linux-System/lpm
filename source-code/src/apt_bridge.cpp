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
#include <apt-pkg/acquire-item.h>
#include <apt-pkg/packagemanager.h>
#include <apt-pkg/sourcelist.h>
#include <apt-pkg/install-progress.h>
#include <apt-pkg/strutl.h>
#include <iostream>
#include <sstream>
#include <algorithm>
#include <cctype>
#include <unistd.h>
#include <vector>
#include <cmath>
#include <regex>

extern pkgSystem *_system;
extern Configuration *_config;

// --- Helper for Case Insensitive Search ---
bool contains_ignore_case(const std::string &haystack, const std::string &needle) {
    if (needle.empty()) return true;
    auto it = std::search(
        haystack.begin(), haystack.end(),
                          needle.begin(), needle.end(),
                          [](unsigned char ch1, unsigned char ch2) {
                              return std::tolower(ch1) == std::tolower(ch2);
                          }
    );
    return (it != haystack.end());
}

namespace legendary {

    // --- Rust Bridge Status ---
    // Inherits from global pkgAcquireStatus
    class LegendaryAcquireStatus : public pkgAcquireStatus {
    public:
        LegendaryAcquireStatus() : pkgAcquireStatus() {}

        bool Pulse(pkgAcquire *Owner) override {
            bool ret = pkgAcquireStatus::Pulse(Owner);

            double percent = 0.0;
            if (TotalBytes > 0) {
                percent = (double(CurrentBytes + CurrentItems) * 100.0) / double(TotalBytes + TotalItems);
            }

            std::string status = "Downloading";
            // Calls legendary::raw_progress_report
            raw_progress_report((float)percent, rust::String(status));

            return ret;
        }

        bool MediaChange(std::string /*Media*/, std::string /*Drive*/) override {
            return false;
        }
    };

    class LegendaryInstallProgress : public APT::Progress::PackageManager {
    public:
        LegendaryInstallProgress() : APT::Progress::PackageManager() {}

        bool StatusChanged(std::string PackageName, unsigned int StepsDone,
                           unsigned int TotalSteps, std::string HumanReadableAction) override {
                               APT::Progress::PackageManager::StatusChanged(PackageName, StepsDone, TotalSteps, HumanReadableAction);

                               float percent = 0.0;
                               if (TotalSteps > 0) {
                                   percent = ((float)StepsDone / (float)TotalSteps) * 100.0;
                               }

                               std::string msg = HumanReadableAction + ": " + PackageName;
                               raw_progress_report(percent, rust::String(msg));

                               return true;
                           }

                           void Error(std::string const &Err, int /*Steps*/, int /*Size*/, std::string const &/*Pkg*/) {
                               // Log to stderr for debugging, but _error stack handles the UI return
                               std::cerr << "\n[DPKG ERROR] " << Err << std::endl;
                               // Ensure it's in the error stack so Rust sees it
                               _error->Error("%s", Err.c_str());
                           }

                           void ConffilePrompt(std::string const &Text, std::vector<std::string> const &/*Options*/,
                                               int &/*Content*/, std::string const &/*Pkg*/, std::string const &/*File*/) {
                               std::cout << "\n[CONFIG PROMPT] " << Text << std::endl;
                                               }
    };

    class AptClient::Impl {
    public:
        pkgCacheFile CacheFile;

        Impl() {}

        void Init() {
            if (_config == nullptr) {
                _config = new Configuration;
            }
            if (_system == nullptr) {
                pkgInitConfig(*_config);
                pkgInitSystem(*_config, _system);
            }

            _config->Set("DPkg::Options::", "--force-confdef");
            _config->Set("DPkg::Options::", "--force-confold");
            _config->Set("quiet", 0);
            _config->Set("APT::Get::Assume-Yes", "true");
        }

        void Open(bool with_lock) {
            if (CacheFile.Open(nullptr, with_lock) == false) {
                // Errors in _error
            }
        }
    };

    AptClient::AptClient() : impl(new Impl()) {}
    AptClient::~AptClient() {}

    void AptClient::init(bool with_lock) {
        impl->Init();
        impl->Open(with_lock);
    }

    void AptClient::update_cache() {
        if (impl->CacheFile.BuildCaches(nullptr, true) == false) {
            return;
        }
        pkgSourceList *List = impl->CacheFile.GetSourceList();
        if (List == nullptr) return;

        LegendaryAcquireStatus Stat;
        raw_phase_report(rust::String("Network"));
        ListUpdate(Stat, *List, 0);
    }

    PkgInfo fill_info(pkgCache::PkgIterator P, pkgCacheFile &CacheFile) {
        PkgInfo info;
        info.name = rust::String(P.Name());
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
            info.version = rust::String(V.VerStr());
            info.size = V->Size;
            info.section = V.Section() ? rust::String(V.Section()) : rust::String("unknown");
        } else {
            info.version = rust::String("N/A");
            info.size = 0;
            info.section = rust::String("unknown");
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

        pkgRecords Recs(impl->CacheFile);

        std::regex regex_term;
        bool use_regex = false;
        try {
            regex_term = std::regex(q, std::regex_constants::icase);
            use_regex = true;
        } catch (...) {
            use_regex = false;
        }

        for (; !P.end(); ++P) {
            if (P.VersionList().end()) continue;

            std::string name = P.Name();
            bool match = false;

            if (use_regex) {
                if (std::regex_search(name, regex_term)) match = true;
            } else {
                if (contains_ignore_case(name, q)) match = true;
            }

            if (!match) {
                pkgCache::VerIterator V = P.VersionList();
                if (!V.end()) {
                    pkgCache::DescIterator D = V.DescriptionList();
                    if (!D.end()) {
                        pkgRecords::Parser &Parser = Recs.Lookup(D.FileList());
                        std::string desc = Parser.ShortDesc();
                        if (use_regex) {
                            if (std::regex_search(desc, regex_term)) match = true;
                        } else {
                            if (contains_ignore_case(desc, q)) match = true;
                        }
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

        details.version = rust::String(V.VerStr());
        details.section = V.Section() ? rust::String(V.Section()) : rust::String("");
        details.maintainer = rust::String("Unknown");
        details.installed_size = V->InstalledSize;
        details.download_size = V->Size;

        pkgRecords Recs(impl->CacheFile);
        pkgRecords::Parser &Parser = Recs.Lookup(V.DescriptionList().FileList());
        details.description = rust::String(Parser.LongDesc());

        for (pkgCache::DepIterator D = V.DependsList(); !D.end(); ++D) {
            if (D->Type == pkgCache::Dep::Depends) {
                details.dependencies.push_back(rust::String(D.TargetPkg().Name()));
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
            APT::Upgrade::Upgrade(*impl->CacheFile.GetDepCache(), APT::Upgrade::FORBID_REMOVE_PACKAGES | APT::Upgrade::FORBID_INSTALL_NEW_PACKAGES);
    }

    void AptClient::mark_full_upgrade() {
        if (impl->CacheFile.GetDepCache())
            APT::Upgrade::Upgrade(*impl->CacheFile.GetDepCache(), 0);
    }

    void AptClient::mark_autoremove() {
        if (!impl->CacheFile.GetDepCache()) return;

        pkgDepCache *DepCache = impl->CacheFile.GetDepCache();

        pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->PkgBegin();
        for (; !P.end(); ++P) {
            if ((*DepCache)[P].Garbage) {
                DepCache->MarkDelete(P, false);
            }
        }
    }

    TransactionSummary AptClient::get_transaction_changes() {
        TransactionSummary summary;
        if (!impl->CacheFile.GetDepCache()) return summary;

        pkgDepCache *DepCache = impl->CacheFile.GetDepCache();
        pkgCache::PkgIterator P = impl->CacheFile.GetPkgCache()->PkgBegin();

        for (; !P.end(); ++P) {
            pkgDepCache::StateCache &State = (*DepCache)[P];

            if (State.NewInstall() || State.ReInstall()) {
                summary.to_install.push_back(rust::String(P.Name()));
            } else if (State.Delete()) {
                summary.to_remove.push_back(rust::String(P.Name()));
            } else if (State.Upgrade()) {
                summary.to_upgrade.push_back(rust::String(P.Name()));
            }
        }
        return summary;
    }

    bool AptClient::resolve() {
        return impl->CacheFile.GetDepCache() ? pkgFixBroken(*impl->CacheFile.GetDepCache()) : false;
    }

    int64_t AptClient::get_download_size() const {
        return impl->CacheFile.GetDepCache() ? impl->CacheFile.GetDepCache()->DebSize() : 0;
    }

    bool AptClient::commit_changes() {
        if (impl->CacheFile.GetPkgCache() == nullptr || impl->CacheFile.GetDepCache() == nullptr) {
            _error->Error("Internal Error: Cache empty during commit.");
            return false;
        }

        if (_error->PendingError()) {
            // Return false so Rust can read the pending error
            return false;
        }

        pkgPackageManager *PM = new pkgPackageManager(impl->CacheFile.GetDepCache());
        if (PM == nullptr) {
            _error->Error("Failed to create Package Manager");
            return false;
        }

        pkgRecords Recs(impl->CacheFile);
        pkgSourceList *List = impl->CacheFile.GetSourceList();

        LegendaryAcquireStatus DownloadStat;
        pkgAcquire Fetcher(&DownloadStat);

        if (!PM->GetArchives(&Fetcher, List, &Recs) || !PM->FixMissing()) {
            // Errors are in _error
            delete PM;
            return false;
        }

        raw_phase_report(rust::String("Acquire"));
        auto result = Fetcher.Run();

        if (result != pkgAcquire::Continue) {
            // Errors are in _error
            delete PM;
            return false;
        }

        raw_phase_report(rust::String("System"));
        LegendaryInstallProgress *progress = new LegendaryInstallProgress();

        pkgPackageManager::OrderResult Res = PM->DoInstall(progress);

        delete progress;
        delete PM;

        if (Res == pkgPackageManager::Failed) {
            // Errors should be in _error
            return false;
        }
        if (Res == pkgPackageManager::Incomplete) {
            // Errors should be in _error
            return false;
        }

        return true;
    }

    void AptClient::clean_cache() {
        pkgAcquire Fetcher;
        Fetcher.Clean("");
    }

    rust::String AptClient::get_last_error() {
        std::string full_msg;
        std::string msg;
        while (!_error->empty()) {
            if (_error->PopMessage(msg)) {
                if (!full_msg.empty()) full_msg += "\n";
                full_msg += msg;
            } else {
                break;
            }
        }
        return rust::String(full_msg);
    }

    std::unique_ptr<AptClient> new_apt_client() {
        return std::make_unique<AptClient>();
    }

} // namespace legendary
