pub mod adapters;
pub mod app;
pub mod domain;
pub mod error;
pub mod interfaces;
pub mod ports;
#[doc(hidden)]
pub mod testing;
pub mod util;

// Earlier module paths, kept for the integration tests and other callers.
pub use adapters::agents::claude;
pub use adapters::store::fs as store;
pub use interfaces::{cli, web};
