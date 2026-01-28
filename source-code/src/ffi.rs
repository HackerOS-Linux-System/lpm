use cxx::{UniquePtr, CxxString};
use std::pin::Pin;

#[cxx::bridge]
pub mod ffi {
    struct PkgInfo {
        name: String,
        section: String,
        version: String,
        current_state: i64,
        size: i64,
    }

    struct PkgDetails {
        name: String,
        version: String,
        section: String,
        maintainer: String,
        description: String,
        installed_size: i64,
        download_size: i64,
        dependencies: Vec<String>,
    }

    unsafe extern "C++" {
        include!("legendary/src/apt_bridge.h");

        type AptClient;

        fn new_apt_client() -> UniquePtr<AptClient>;
        fn init(self: Pin<&mut AptClient>);

        fn list_all(self: Pin<&mut AptClient>) -> Vec<PkgInfo>;
        fn search(self: Pin<&mut AptClient>, term: String) -> Vec<PkgInfo>;
        fn find_package(self: Pin<&mut AptClient>, name: String) -> PkgInfo;
        fn get_package_details(self: Pin<&mut AptClient>, name: String) -> PkgDetails;

        fn mark_install(self: Pin<&mut AptClient>, name: String);
        fn mark_remove(self: Pin<&mut AptClient>, name: String);
        fn mark_auto(self: Pin<&mut AptClient>, name: String, is_auto: bool);
        fn mark_upgrade(self: Pin<&mut AptClient>);

        fn resolve(self: Pin<&mut AptClient>) -> bool;
        fn get_download_size(self: &AptClient) -> i64;
        fn commit_changes(self: Pin<&mut AptClient>) -> bool;
        fn clean_cache(self: Pin<&mut AptClient>);
    }
}
