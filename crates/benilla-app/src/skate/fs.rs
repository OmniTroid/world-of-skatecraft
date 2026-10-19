//! The skate mode's own files: `std::fs` natively. On the web they are URL paths on the page's
//! origin (its server serves `skate-data/` and `skate-audio/`), read through `benilla_mpq::fs`.

use std::io;
use std::path::Path;

pub(super) fn read(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::fs::read(path);
    #[cfg(target_arch = "wasm32")]
    {
        use std::io::Read;
        let mut bytes = Vec::new();
        benilla_mpq::fs::File::open(path)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

pub(super) fn is_file(path: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return path.is_file();
    #[cfg(target_arch = "wasm32")]
    return benilla_mpq::fs::File::open(path).is_ok();
}
