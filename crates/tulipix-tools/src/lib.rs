//! Tools section — the shared Rust core behind both the GUI Tools sidebar and
//! the `tulipix` CLI. Every operation (rename, merge, split, compress,
//! convert, trim, extract, metadata, thumbnail, watermark, normalize,
//! transcribe, download, burn-subs, resize, PDF, hash, folder-diff) is a
//! pure-logic argv/plan builder or parser here; the GUI and CLI both submit
//! them to a single persisted job queue (`tools.db`) so workers, progress, and
//! retries are identical across front-ends.

pub mod schema;

pub mod burn_subs;
pub mod cli;
pub mod compress_audio;
pub mod compress_photo;
pub mod compress_video;
pub mod convert;
pub mod download;
pub mod download_live;
pub mod download_playlist;
pub mod download_routing;
pub mod exec;
pub mod extract;
pub mod folder_diff;
pub mod hash;
pub mod merge;
pub mod metadata;
pub mod normalize;
pub mod pdf;
pub mod queue;
pub mod rename;
pub mod resize;
pub mod section;
pub mod split;
pub mod thumbnail;
pub mod transcribe;
pub mod trim;
pub mod watermark;
pub mod ytdlp_update;
