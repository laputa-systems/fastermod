#![allow(dead_code)]

// Keep the executable and the benchmark target on the same implementation. The
// benchmark-only module in main.rs exposes narrow wrappers around production
// paths without making the application itself depend on benchmark code.
include!("main.rs");
