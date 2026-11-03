//! Central child-process spawn helper.
//!
//! Every child this app spawns must go through [`command`]. The release
//! binary is built with `windows_subsystem = "windows"` (no console), so a
//! plain spawn of a console program (git, cmd, ssh-keygen, …) allocates a
//! fresh console window and steals focus from the app on every spawn.
//! `CREATE_NO_WINDOW` suppresses that. In dev the parent console is
//! inherited either way, which is why the flash only shows up in release
//! builds.

use std::process::Command;

/// `Command::new(program)` with `CREATE_NO_WINDOW` set on Windows (no-op
/// elsewhere).
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    // The `mut` is consumed only by the Windows block below.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
