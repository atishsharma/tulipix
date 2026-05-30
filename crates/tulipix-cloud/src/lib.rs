//! Cloud section — rclone driver. Config CRUD shelling to the bundled rclone
//! binary, FUSE/WinFsp mounts with auto-remount, union/merge remotes, a
//! no-mount HTTP fallback, one-way + bisync with throttle/schedule, config
//! encrypted at rest (password in the OS keychain), a Personal Vault, per-file
//! revisions, share links, a recycle bin, a ransomware heuristic, a native
//! remote-tree browser + previews + libmpv streaming, selective sync, the
//! shared-with-me inbox, drag-and-drop upload, restore-by-date, and full
//! right-click context-menu parity.
//!
//! rclone is invoked as a subprocess; the testable surface here is argv
//! construction, output parsing, and the `cloud.db` bookkeeping.

pub mod schema;

pub mod browse;
pub mod context;
pub mod encrypt;
pub mod http;
pub mod mount;
pub mod preview;
pub mod ransomware;
pub mod recycle;
pub mod remotes;
pub mod restore_snapshot;
pub mod selective;
pub mod share;
pub mod shared_inbox;
pub mod stream;
pub mod sync;
pub mod union;
pub mod upload_dnd;
pub mod vault;
pub mod versions;
