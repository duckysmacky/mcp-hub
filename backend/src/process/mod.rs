//! Everything needed to run a child safely. All MCP children are built here.

/// The only `PATH` a child ever sees; user values can't change it.
pub const GATEWAY_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
