pub mod adapters;
pub mod app;
pub mod domain;
pub mod error;
pub mod interfaces;
pub mod ports;
/// Fakes and fixtures for tests; built only for tests and with the `testing` feature (which the
/// integration tests in `tests/` turn on), never into a release.
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub mod testing;
pub mod util;
