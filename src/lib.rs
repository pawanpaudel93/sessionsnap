pub mod adapters;
pub mod app;
pub mod cli;
pub mod core;
pub mod error;
pub mod ports;
#[doc(hidden)]
pub mod testing;
pub mod web;

// Earlier module paths, kept for the integration tests and other callers.
pub use adapters::agents::claude;
pub use adapters::store::fs as store;
