//! Brewery's single switch for Zed's cloud. A leaf crate, so low-level Zed
//! crates like `client` can depend on it without dependency cycles.

/// Brewery never talks to Zed's cloud: no Zed account sign-in, collaboration,
/// Zed AI, telemetry upload or Zed update checks. Hook-ins in Zed's crates
/// check this instead of each carrying its own switch.
pub const ZED_CLOUD_ENABLED: bool = false;
