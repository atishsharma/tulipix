//! Books section, extracted from tulipix-app.
//!
//! - [`books`] — low-level parsing/rasterisation (EPUB/OPF/FB2/MOBI, poppler
//!   `pdftoppm`/`pdfinfo` raster, DjVu, comic archives, cover extraction, ingest).
//! - [`reader`] — the native reader + library grid glue + every `on_book_*`
//!   callback. `tulipix-app` calls [`wire_books`] once at startup.
//!
//! Public surface kept intentionally small: the section wiring plus the three
//! entry points other sections reach into (open a book, refresh the grid, ingest
//! a freshly-scanned file).

mod books;
mod reader;

pub use books::{ingest_file, Ingest};
pub use reader::{open_book_path, refresh_books, wire_books};
