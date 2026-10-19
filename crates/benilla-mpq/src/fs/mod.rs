//! The files under the archives: `std::fs` natively. On the web, paths are URL paths on the page's
//! origin, read with synchronous Range requests through a block cache, and a directory is the
//! server's JSON listing of it; in local-folder mode the page maps those paths to the picked
//! files' blob URLs instead.

use std::io;
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::File;

#[cfg(target_arch = "wasm32")]
pub use web::File;

#[cfg(target_arch = "wasm32")]
mod web;

/// The file's length in bytes.
#[cfg(not(target_arch = "wasm32"))]
pub fn len(file: &File) -> io::Result<u64> {
    file.metadata().map(|m| m.len())
}

#[cfg(target_arch = "wasm32")]
pub fn len(file: &File) -> io::Result<u64> {
    Ok(file.len())
}

/// Whether `path` is a directory.
pub fn is_dir(path: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return path.is_dir();
    #[cfg(target_arch = "wasm32")]
    return web::list(path).is_some();
}

/// The names of the files directly in `path`, unsorted.
pub fn list_files(path: &Path) -> io::Result<Vec<String>> {
    #[cfg(not(target_arch = "wasm32"))]
    return Ok(std::fs::read_dir(path)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // `path().is_file()` follows symlinks (`read_dir`'s file_type doesn't).
            entry.path().is_file().then(|| entry.file_name())
        })
        .filter_map(|name| name.into_string().ok())
        .collect());
    #[cfg(target_arch = "wasm32")]
    return web::list(path).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound));
}
