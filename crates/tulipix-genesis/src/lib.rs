//! Book search and download against Library Genesis mirrors.
//!
//! The network and parsing core is a port of Tomesole (MIT) — see
//! `LICENSE-TOMESOLE`. Its terminal UI is not ported; what came across is the
//! part that is genuinely hard to get right, tests included:
//!
//! - [`net`]    — a validated HTTP client with an SSRF guard on every redirect
//! - [`libgen`] — search URLs, results-table parsing, download-link resolution
//! - [`mirror`] — finding a mirror whose *search* works, not just its homepage
//! - [`download`] — streaming to disk with MD5 verification
//!
//! Everything here is **blocking**. The app bridges to it with one
//! `spawn_blocking` in `tulipix-sec-genesis` rather than porting `net.rs` to
//! reqwest, because that module deliberately validates redirects with ureq's
//! own re-exported `http::Uri` parser — the same parser that opens the
//! connection — and swapping parsers would reopen the gap it closes.

pub mod cover;
pub mod details;
pub mod download;
pub mod error;
pub mod fsutil;
pub mod history;
pub mod html;
pub mod libgen;
pub mod md5;
pub mod mirror;
pub mod model;
pub mod net;
pub mod query;

use std::path::{Path, PathBuf};

use ureq::http::Uri;

use crate::error::Result;
use crate::mirror::{Pool, ResolveOptions};
use crate::model::{Book, SearchQuery};
use crate::net::{Http, NetPolicy};

/// Reported alongside every result set so the UI can name the mirror that
/// actually answered. Mirrors fail constantly and failover is otherwise silent.
pub type Mirror = String;

/// A search or download, plus the host that served it.
pub struct Served<T> {
    pub value: T,
    /// Bare host, for the status chip — `libgen.li`.
    pub mirror: Mirror,
    /// The full base the request actually went to, scheme and all. The chip
    /// wants the host; anyone asking "where are these books coming from" wants
    /// something they can paste into a browser.
    pub url: String,
}

/// The blocking facade the app drives.
///
/// Holds the HTTP client and the resolved mirror pool between calls, so the
/// second search in a session does not re-probe.
pub struct GenesisService {
    http: Http,
    policy: NetPolicy,
    /// `None` until the first successful resolve.
    pool: Option<Pool>,
    /// Mirrors the user named in Settings. Non-empty means "use these, in this
    /// order, and do not probe".
    explicit: Vec<String>,
    dest: PathBuf,
}

impl GenesisService {
    pub fn new(explicit: Vec<String>, dest: PathBuf) -> Result<Self> {
        let policy = NetPolicy::default();
        Ok(Self { http: Http::new(policy)?, policy, pool: None, explicit, dest })
    }

    pub fn dest(&self) -> &Path {
        &self.dest
    }

    pub fn set_dest(&mut self, dest: PathBuf) {
        self.dest = dest;
    }

    /// Replace the configured mirror list. Drops the resolved pool so the next
    /// search honours the new setting instead of the old ordering.
    pub fn set_mirrors(&mut self, explicit: Vec<String>) {
        if explicit != self.explicit {
            self.explicit = explicit;
            self.pool = None;
        }
    }

    /// Whether a mirror pool is already resolved.
    ///
    /// The UI asks so it can tell "searching" from "probing": a cold start runs
    /// a real query against every seed mirror and takes seconds, and a spinner
    /// that does not say so reads as a hang.
    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }

    /// Forget both the in-memory pool and the on-disk ranking, so the next
    /// search probes from scratch.
    pub fn refresh_mirrors(&mut self) {
        self.pool = None;
        let _ = mirror::clear_cache();
    }

    /// Resolve a mirror pool, reusing one already found.
    ///
    /// `on_status` is called with human-readable progress. A cold probe runs a
    /// real search against every seed concurrently and can take ten seconds or
    /// more — without something to say during it, that reads as a hang.
    ///
    /// Returns nothing rather than the pool: handing back a reference derived
    /// from `&mut self` would freeze the whole struct, and every caller needs
    /// `self.http` at the same time. Callers read [`Self::pool`] afterwards,
    /// which is an ordinary shared borrow.
    pub fn ensure_pool(&mut self, on_status: &dyn Fn(&str)) -> Result<()> {
        if self.pool.is_none() {
            let pool = mirror::resolve(
                &self.policy,
                ResolveOptions { explicit: &self.explicit, refresh: false, progress: on_status },
            )?;
            self.pool = Some(pool);
        }
        Ok(())
    }

    /// The resolved pool. Only `None` before the first [`Self::ensure_pool`].
    fn pool(&self) -> Result<&Pool> {
        self.pool.as_ref().ok_or_else(|| err!("no mirror has been resolved yet"))
    }

    /// Run `op` against the pool, and throw the pool away if nothing answered.
    ///
    /// Every mirror failing at once almost never means Libgen is gone — it
    /// means the ranking we are holding has gone stale, or the shared gateway
    /// in front of the `.li` family had a bad minute. Without this, the ranking
    /// survives in memory for the life of the process and on disk for the
    /// cache's six hours, so one bad minute replays the same dead list for the
    /// rest of the day while the site loads fine in a browser. Dropping it
    /// costs one re-probe on the next attempt and lets the section heal itself
    /// instead of waiting for the user to find the refresh button.
    fn on_each_mirror<T>(
        &mut self,
        on_status: &dyn Fn(&str),
        mut op: impl FnMut(&Http, &Uri) -> Result<T>,
        on_retry: impl FnMut(&Uri, &str),
    ) -> Result<(T, Uri)> {
        self.ensure_pool(on_status)?;
        // Scoped so both shared borrows of `self` end before the mutation.
        let result = {
            let http = &self.http;
            self.pool()?.try_each(|base| op(http, base), on_retry)
        };
        if result.is_err() {
            self.refresh_mirrors();
        }
        result
    }

    /// Run a search, trying each mirror in turn until one answers.
    ///
    /// Client-side filters (`extension`, `language`) are applied here because
    /// mirrors cannot express them in the query string — which is also why
    /// `SearchQuery::page_size` over-fetches when either is set.
    ///
    /// Everything that survived the filters comes back, not just `limit` of it:
    /// the over-fetch means one request usually holds several screens' worth,
    /// and "load more" should spend those before asking the mirror for another
    /// page. `limit` still decides how large a page to request.
    pub fn search(
        &mut self,
        query: &SearchQuery,
        on_status: &dyn Fn(&str),
    ) -> Result<Served<Vec<Book>>> {
        let (books, base) = self.on_each_mirror(
            on_status,
            |http, base| libgen::search(http, base, query),
            |base, why| {
                let host = net::host_of(base).unwrap_or_else(|_| base.to_string());
                on_status(&format!("{host} failed ({why}) — trying the next mirror"));
            },
        )?;

        let kept: Vec<Book> = books.into_iter().filter(|b| query.matches(b)).collect();
        Ok(Served { value: kept, mirror: host_label(&base), url: base.to_string() })
    }

    /// Fetch one book's cover, from the on-disk cache when it is there.
    ///
    /// `Ok(None)` means the record genuinely has no cover — an answer that is
    /// cached too, so a book without art costs one lookup ever, not one per
    /// search that turns it up.
    pub fn cover(&mut self, book: &Book, on_status: &dyn Fn(&str)) -> Result<Option<PathBuf>> {
        // The cache answers without a mirror, which matters: covers are asked
        // for a screenful at a time, and a cold pool would otherwise make the
        // first one wait on a full probe.
        if let Some(hit) = cover::cached(&book.md5) {
            return Ok(hit);
        }
        // Silent on retry: covers are fetched a screenful at a time, and one
        // status line per mirror per book would bury everything the status
        // strip exists to say.
        let (found, _) =
            self.on_each_mirror(on_status, |http, base| cover::fetch(http, base, book), |_, _| {})?;
        Ok(found)
    }

    /// Fetch a record page's full metadata.
    ///
    /// Always goes to the network; the caller owns the cache, because the same
    /// call is what a "refresh" button needs.
    pub fn details(
        &mut self,
        book: &Book,
        on_status: &dyn Fn(&str),
    ) -> Result<Served<details::Details>> {
        let (found, base) = self.on_each_mirror(
            on_status,
            |http, base| details::fetch(http, base, book),
            |base, why| {
                let host = net::host_of(base).unwrap_or_else(|_| base.to_string());
                on_status(&format!("{host} could not describe that book ({why})"));
            },
        )?;
        Ok(Served { value: found, mirror: host_label(&base), url: base.to_string() })
    }

    /// Resolve a record to a live URL and stream it to disk.
    ///
    /// Two hops on the mirror — the interstitial that mints a single-use key,
    /// then the file itself — followed by an MD5 check before the `.part` is
    /// renamed into place.
    pub fn download(
        &mut self,
        book: &Book,
        opts: &download::Options,
        report: download::Report<'_>,
        on_status: &dyn Fn(&str),
    ) -> Result<Served<download::Outcome>> {
        let md5 = book.md5.clone();
        let (resolved, base) = self.on_each_mirror(
            on_status,
            |http, base| libgen::resolve_download(http, base, &md5),
            |base, why| {
                let host = net::host_of(base).unwrap_or_else(|_| base.to_string());
                on_status(&format!("{host} would not hand over a link ({why})"));
            },
        )?;

        // The interstitial carries metadata the results table may have been
        // missing — an absent extension there means the filename could not be
        // decided up front, and with it resume and skip-if-present both work.
        let merged = merge(book, &resolved.book);
        on_status(&format!("downloading from {}", host_label(&base)));
        let outcome = download::fetch(&self.http, &resolved.url, &merged, opts, report)?;
        Ok(Served { value: outcome, mirror: host_label(&base), url: base.to_string() })
    }
}

/// Prefer what the catalogue row said, fall back to the interstitial.
///
/// The row is the record the user actually chose from, so its title and author
/// win; the interstitial is consulted only where the row was blank.
fn merge(row: &Book, ads: &Book) -> Book {
    fn or(a: &Option<String>, b: &Option<String>) -> Option<String> {
        a.clone().filter(|s| !s.trim().is_empty()).or_else(|| b.clone())
    }
    Book {
        md5: row.md5.clone(),
        title: if row.title.trim().is_empty() { ads.title.clone() } else { row.title.clone() },
        authors: or(&row.authors, &ads.authors),
        publisher: or(&row.publisher, &ads.publisher),
        year: or(&row.year, &ads.year),
        language: or(&row.language, &ads.language),
        pages: or(&row.pages, &ads.pages),
        size_bytes: row.size_bytes.or(ads.size_bytes),
        extension: or(&row.extension, &ads.extension),
        file_id: or(&row.file_id, &ads.file_id),
    }
}

fn host_label(base: &Uri) -> String {
    net::host_of(base).unwrap_or_else(|_| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Book {
        Book {
            md5: "abc".into(),
            title: "Chosen Title".into(),
            authors: Some("Row Author".into()),
            extension: None,
            ..Default::default()
        }
    }

    #[test]
    fn the_row_the_user_picked_wins_over_the_interstitial() {
        let ads = Book {
            md5: "abc".into(),
            title: "Scraped Title".into(),
            authors: Some("Ads Author".into()),
            extension: Some("epub".into()),
            ..Default::default()
        };
        let merged = merge(&row(), &ads);
        assert_eq!(merged.title, "Chosen Title");
        assert_eq!(merged.authors.as_deref(), Some("Row Author"));
        // The one field the row did not have is taken from the interstitial —
        // without it the filename cannot be decided before the transfer starts.
        assert_eq!(merged.extension.as_deref(), Some("epub"));
    }

    #[test]
    fn a_blank_row_field_falls_through_rather_than_winning_as_empty() {
        let mut r = row();
        r.title = "   ".into();
        r.authors = Some("".into());
        let ads = Book {
            md5: "abc".into(),
            title: "Scraped Title".into(),
            authors: Some("Ads Author".into()),
            ..Default::default()
        };
        let merged = merge(&r, &ads);
        assert_eq!(merged.title, "Scraped Title");
        assert_eq!(merged.authors.as_deref(), Some("Ads Author"));
    }
}
