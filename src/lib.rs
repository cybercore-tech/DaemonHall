//! Shared between the web server (`daemonhall`) and the root helper
//! (`daemonhall-unitctl`): unit-file generation and the rules for which
//! system units DaemonHall may control.

pub mod allow;
pub mod unitgen;

/// Where `packaging/install-root.sh` puts the helper; the polkit action is
/// bound to exactly this path.
pub const HELPER_PATH: &str = "/usr/local/bin/daemonhall-unitctl";
pub const POLKIT_ACTION: &str = "tech.cybercore.daemonhall.unitctl";
pub const DEFAULT_PORT: u16 = 8767;
