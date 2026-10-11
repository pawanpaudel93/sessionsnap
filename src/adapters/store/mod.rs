//! Where snapshots are kept. Today: JSON files on disk (`fs`).
//!
//! Layer rule: an adapter; may use `crate::ports` and `crate::core`, never `app` or `interfaces`.

pub mod fs;

pub use fs::Store;
