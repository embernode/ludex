//! Readers for the pieces of `/proc/<pid>/*` the non-launcher detector
//! consults.
//!
//! Each submodule is a thin, total wrapper around a particular file or
//! directory under `/proc/<pid>`. Every read can fail (the process can
//! exit between a `stat` and an `open`, permission bits can hide a file
//! on hardened distros); callers treat `io::Error` as "no information,
//! skip this probe".
//!
//! The parsers are pure `&str`/`&[u8]` functions so they stay testable
//! without a live process backing them.

pub mod environ;
pub mod exe;
pub mod fdinfo;
pub mod fds;
pub mod maps;
pub mod tree;
