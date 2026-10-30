//! Remote device projects over SSH.
//!
//! Architecture: a device profile (`profiles`) describes a connection; a
//! russh-backed session pool (`transport`) multiplexes exec/SFTP/shell
//! channels per device; the remote git engine (`engine`) runs `git` on the
//! device over exec and parses porcelain output into the same DTO types the
//! local libgit2 engine produces; file operations go over SFTP
//! (`workspace`). Repos on devices are first-class tabs — see
//! `repo::RepoHandle::backend`.
//!
//! Security invariants: every user-controlled value interpolated into a
//! remote command passes through [`quoting::shell_quote`]; secrets live only
//! in the OS keyring; host keys are trust-on-first-use with a hard warning
//! on change (`hostkeys`); FE-supplied remote paths are validated
//! (`workspace::validate_rel_path`).

pub mod actions;
pub mod commands;
pub mod engine;
pub mod hostkeys;
pub mod parsers;
pub mod polling;
pub mod profiles;
pub mod prompts;
pub mod quoting;
pub mod term;
pub mod transport;
pub mod workspace;
