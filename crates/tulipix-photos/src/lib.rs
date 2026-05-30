//! Photos section — scan, EXIF, thumbnails, timeline, folder view, viewer,
//! albums, smart albums, FTS5 search, dedup, trash, archive, star, the AI
//! sub-tree (models + faces + clusters + people + merge + tags + CLIP + EP
//! + background indexer + best-frame), non-destructive editor (adjust /
//! curves / crop / filters / tint / text / sharpen / motion-blur / redeye /
//! enhance / color-pop + AI heal/sky/upscale/colorize hooks + export),
//! memories (on-this-day + trips), map (MBTiles + pin clusters), places
//! (offline reverse geocode), burst detect → GIF/MP4, collages (template +
//! picture pile), slideshow plan, and the HTML web gallery export.

pub mod ai;
pub mod albums;
pub mod archive;
pub mod burst;
pub mod codecs;
pub mod collages;
pub mod date_fix;
pub mod dedup;
pub mod editor;
pub mod exif;
pub mod exif_write;
pub mod export_html;
pub mod folders;
pub mod hdr_bracket;
pub mod heic_convert;
pub mod live_photos;
pub mod map;
pub mod memories;
pub mod panorama;
pub mod places;
pub mod scan;
pub mod schema;
pub mod search;
pub mod slideshow;
pub mod smart_albums;
pub mod spatial;
pub mod stacks;
pub mod star;
pub mod thumbs;
pub mod timeline;
pub mod trash;
pub mod viewer;
