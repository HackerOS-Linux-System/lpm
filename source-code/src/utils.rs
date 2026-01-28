use nix::unistd::Uid;

pub fn is_root() -> bool {
    Uid::effective().is_root()
}

pub fn human_bytes(size: i64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut s = size as f64;
    let mut i = 0;
    while s >= 1024.0 && i < units.len() - 1 {
        s /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", s, units[i])
}
