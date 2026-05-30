//! tulipix-sync — Account-mode sync engine + WAN streaming endpoint.
//!
//! The sync server itself is the optional Go service; this crate is the Rust
//! client/engine side: the Account-mode toggle, OAuth deep-link flow, the
//! batch-upsert + watermark-pull engine, user invitations, the activity feed,
//! shared albums + members, 2FA/TOTP, and the admin audit log. Relational
//! state lives in a local `sync.db` mirror; pure protocol logic (OAuth URLs,
//! TOTP, watermark math) is dependency-light and unit-tested.

pub mod schema;

pub mod account_toggle;
pub mod activity;
pub mod audit_log;
pub mod engine;
pub mod invitations;
pub mod oauth;
pub mod remote_stream;
pub mod shared_albums;
pub mod totp;
