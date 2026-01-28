fn main() {
    cxx_build::bridge("src/main.rs")
        .file("src/apt_bridge.cpp")
        .flag_if_supported("-std=c++17")
        .compile("legendary-apt");

    println!("cargo:rustc-link-lib=apt-pkg");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=src/apt_bridge.cpp");
    println!("cargo:rerun-if-changed=src/apt_bridge.h");
}
