#pragma once
#include "rust/cxx.h"
#include <memory>
#include <string>
#include <vector>

// Forward decl
namespace legendary {
    class AptClient;
}

// We include the header generated from ffi.rs
// It now defines namespace legendary { ... }
#include "legendary/src/ffi.rs.h"

namespace legendary {

    class AptClient {
    public:
        AptClient();
        ~AptClient();

        void init(bool with_lock);
        void update_cache();

        // Core Logic
        rust::Vec<PkgInfo> list_all();
        rust::Vec<PkgInfo> search(rust::String term);
        PkgInfo find_package(rust::String name);
        PkgDetails get_package_details(rust::String name);

        // Actions
        void mark_install(rust::String name);
        void mark_remove(rust::String name);
        void mark_upgrade();
        void mark_full_upgrade();
        void mark_autoremove();
        void mark_auto(rust::String name, bool is_auto);

        // Transaction Info
        TransactionSummary get_transaction_changes();

        bool resolve();
        int64_t get_download_size() const;
        bool commit_changes();
        void clean_cache();

        // Error handling
        rust::String get_last_error();

    private:
        class Impl;
        std::unique_ptr<Impl> impl;
    };

    std::unique_ptr<AptClient> new_apt_client();

} // namespace legendary
