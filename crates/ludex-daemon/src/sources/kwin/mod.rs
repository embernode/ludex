//! KDE Plasma KWin foreground-window source.
//!
//! See [`KWinForegroundSource`] for the entry point. Internals:
//! - `script.js`: the KWin script installed at daemon startup.
//! - `source.rs`: D-Bus interface, proxy, script loader, and event
//!   pipeline.
//! - `transition.rs`: pure state-transition logic.

mod source;
mod transition;

pub use source::KWinForegroundSource;
