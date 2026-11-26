//! Pool sesi PTY dengan ring buffer, log rolling, batching 12 ms, dan parser vt100.

pub mod ring;
pub mod session;

pub use session::{ExitInfo, ExitWatcher, PtyError, PtyOptions, PtySession, PtySize};
