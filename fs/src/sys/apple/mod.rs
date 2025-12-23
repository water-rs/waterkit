use std::path::PathBuf;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn documents_dir() -> Option<String>;
        fn cache_dir() -> Option<String>;
    }
}

pub fn documents_dir() -> Option<PathBuf> {
    ffi::documents_dir().map(PathBuf::from)
}

pub fn cache_dir() -> Option<PathBuf> {
    ffi::cache_dir().map(PathBuf::from)
}
