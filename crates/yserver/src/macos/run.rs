//! Launch on macOS.

use std::io;

use crate::launch::{LaunchOptions, ShellMode};

/// Run the server in a window, or headless with `--headless`.
///
/// # Errors
///
/// See [`crate::surface::server::run_headless`] and [`super::app::run`].
pub(crate) fn run(opts: LaunchOptions, mode: ShellMode) -> io::Result<()> {
    if opts.headless {
        return crate::surface::server::run_headless(opts);
    }
    match mode {
        ShellMode::Rooted => super::app::run(opts),
        ShellMode::Rootless => Err(io::Error::other(
            "rootless windows on macOS are not implemented yet; use --rooted",
        )),
    }
}
