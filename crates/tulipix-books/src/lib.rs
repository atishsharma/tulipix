//! Books & Comics section — the Calibre/Komga alternative. Scans EPUB / CBZ /
//! CBR / PDF into `books.db`, drives the native Slint reader (paginator,
//! two-page comic spread, manga RTL, image inversion), typography controls,
//! navigation (TOC + chapter scrub + comic thumbnail timeline), local AI
//! text-to-speech, metadata scraping (ComicVine / OpenLibrary / Google Books),
//! reading progress + bookmarks, library views (series / collections / authors
//! / read-state), and continuous webtoon scroll.
//!
//! Proxy model: every book is an `items` row; `book_meta` and the rest key on
//! `item_id`.

pub mod schema;

pub mod continuous;
pub mod library;
pub mod metadata;
pub mod navigation;
pub mod progress;
pub mod reader;
pub mod scan;
pub mod tts;
pub mod typography;
