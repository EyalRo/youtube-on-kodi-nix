//! Debug logging that is compiled in or out, never branched on at run time.
//!
//! A debug build (`RUSTFLAGS="--cfg appliance_debug"`, which the Nix packages set with
//! `debug = true`) turns [`debug!`] into `tracing::debug!` and lifts the default log filter to
//! debug. Any other build expands `debug!` to nothing: no call site, no level check, no
//! arguments evaluated. `info!`, `warn!` and `error!` stay plain `tracing` calls; they are the
//! logs a production box keeps.

pub use tracing;

/// `tracing::debug!` in debug builds, nothing otherwise.
#[cfg(appliance_debug)]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => { $crate::tracing::debug!($($t)*) };
}

/// `tracing::debug!` in debug builds, nothing otherwise.
#[cfg(not(appliance_debug))]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {};
}

/// The filter used when no `*_LOG` variable is set: info for everything, plus debug for the
/// appliance's own crates in a debug build.
pub const fn default_filter() -> &'static str {
    if cfg!(appliance_debug) {
        "info,player=debug,mediad=debug,store=debug,scanner=debug"
    } else {
        "info"
    }
}

/// Whether this binary was built with debug logging.
pub const DEBUG_BUILD: bool = cfg!(appliance_debug);
