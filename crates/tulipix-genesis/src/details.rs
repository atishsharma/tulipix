//! Full metadata for one record, fetched from the mirror's record page.
//!
//! The search table is deliberately thin — title, author, year, size — because
//! it is one request for fifty books. Everything else (series, publisher, ISBN,
//! description, cover) lives on the record page, which is one request per book
//! and so is only fetched when someone actually asks to see a record.
//!
//! The page states its fields as `Label: value<br>` inside a single cell, so
//! parsing splits on the break rather than trying to find a table shape that
//! mirrors do not agree on. A BibTeX block at the foot of the page is used as a
//! fallback, since it is delimited and cannot bleed one field into the next.

use ureq::http::Uri;

use crate::error::Result;
use crate::model::Book;
use crate::net::Http;
use crate::{cover, html};

/// Everything the record page had to say about one book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub md5: String,
    pub title: String,
    pub series: String,
    pub authors: String,
    pub publisher: String,
    pub year: String,
    pub isbn: String,
    pub language: String,
    pub pages: String,
    pub size: String,
    pub extension: String,
    pub description: String,
    /// `src` of the cover on the record page, as served (relative or absolute).
    pub cover_src: String,
    /// The page this came from, so the UI can say where it looked.
    pub source_url: String,
}

/// Fetch and parse a record page.
///
/// The caller decides whether to consult its cache first; this always goes to
/// the network, which is what makes it usable as the "refresh" path too.
pub fn fetch(http: &Http, base: &Uri, book: &Book) -> Result<Details> {
    let url = cover::record_url(base, book)?;
    let page = http.get_text(&url)?;
    let mut details = parse(&page, &book.md5);
    details.source_url = url.to_string();
    // The search row is authoritative for the columns it carried: it is the
    // record the user actually chose from, and the record page states some of
    // them in a looser form.
    if details.title.trim().is_empty() {
        details.title = book.title.clone();
    }
    if details.authors.trim().is_empty() {
        details.authors = book.authors.clone().unwrap_or_default();
    }
    if details.language.trim().is_empty() {
        details.language = book.language.clone().unwrap_or_default();
    }
    if details.pages.trim().is_empty() {
        details.pages = book.pages.clone().unwrap_or_default();
    }
    if details.extension.trim().is_empty() {
        details.extension = book.ext().to_string();
    }
    if details.size.trim().is_empty() {
        details.size = book.size_human();
    }
    Ok(details)
}

/// The labels a record page uses, mapped onto our fields.
///
/// Matched case-insensitively against the start of each `<br>`-delimited line.
const LABELS: [(&str, fn(&mut Details, String)); 10] = [
    ("title:", |d, v| d.title = v),
    ("series:", |d, v| d.series = v),
    ("author(s):", |d, v| d.authors = v),
    ("author:", |d, v| d.authors = v),
    ("publisher:", |d, v| d.publisher = v),
    ("year:", |d, v| d.year = v),
    ("isbn:", |d, v| d.isbn = v),
    ("language:", |d, v| d.language = v),
    ("pages:", |d, v| d.pages = v),
    ("description:", |d, v| d.description = v),
];

/// Parse a record page into [`Details`].
pub fn parse(page: &str, md5: &str) -> Details {
    let mut out = Details { md5: md5.to_ascii_lowercase(), ..Default::default() };

    // `<br>` is the field separator inside the metadata cell. Splitting on it
    // keeps a value containing a label word — "Notes: see Title: ..." — from
    // swallowing the field after it.
    for chunk in split_on_breaks(page) {
        let line = html::text(&chunk);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        for (label, set) in LABELS {
            if let Some(rest) = lower.strip_prefix(label) {
                // Slice the original, not the lowercased copy: the value keeps
                // its own casing. Both strings have the same byte length here
                // because `to_ascii_lowercase` is byte-for-byte.
                let value = trimmed[trimmed.len() - rest.len()..].trim().to_string();
                if !value.is_empty() {
                    set(&mut out, value);
                }
                break;
            }
        }
    }

    // BibTeX fallback for anything the labelled lines did not carry.
    fill_from_bibtex(&mut out, &html::text(page));

    if out.cover_src.is_empty()
        && let Some(src) = cover::pick_cover_src(page)
    {
        out.cover_src = src;
    }
    out
}

/// Take each `<br>`-separated fragment of the page.
fn split_on_breaks(page: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = page;
    loop {
        let lower = rest.to_ascii_lowercase();
        match lower.find("<br") {
            Some(at) => {
                out.push(rest[..at].to_string());
                // Step past the tag itself so the next fragment starts clean.
                let after = rest[at..].find('>').map(|i| at + i + 1).unwrap_or(rest.len());
                rest = &rest[after..];
            }
            None => {
                out.push(rest.to_string());
                return out;
            }
        }
    }
}

/// Fill empty fields from the BibTeX block the page embeds.
fn fill_from_bibtex(out: &mut Details, text: &str) {
    let take = |name: &str| bibtex_field(text, name);
    if out.title.is_empty() && let Some(v) = take("title") {
        out.title = v;
    }
    if out.authors.is_empty() && let Some(v) = take("author") {
        out.authors = v;
    }
    if out.publisher.is_empty() && let Some(v) = take("publisher") {
        out.publisher = v;
    }
    if out.year.is_empty() && let Some(v) = take("year") {
        out.year = v;
    }
    if out.series.is_empty() && let Some(v) = take("series") {
        out.series = v;
    }
    if out.isbn.is_empty() && let Some(v) = take("isbn") {
        out.isbn = v;
    }
}

/// Read one `name = {value}` field out of a BibTeX block.
///
/// Whitespace around `=` is not fixed: mirrors pad the block into columns
/// (`title =     {`), so the needle is the name, and the brace is found after
/// it. Braces nest, so the value ends at the matching close.
pub fn bibtex_field(text: &str, name: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(at) = text[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        // Must be a field name, not the tail of a longer one.
        let preceded_ok = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || c == ',' || c == '{');
        if !preceded_ok {
            continue;
        }
        let rest = text[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('{') else { continue };

        let mut depth = 1usize;
        let mut value = String::new();
        for c in rest.chars() {
            match c {
                '{' => {
                    depth += 1;
                    value.push(c);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    value.push(c);
                }
                c => value.push(c),
            }
        }
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <tr><td rowspan=2>
          <a href="/covers/3342000/d40695045e16750abb83015f002af0f4.jpg">
            <img src="/covers/3342000/d40695045e16750abb83015f002af0f4.jpg" width=300></a></td>
        <td>Title: Biological N fixation in forage-livestock systems<br>
        Series: ASA special publication 28<br>
        Author(s): Hoveland, C. S.;Kral, David M.<br>
        Publisher: American Society of Agronomy<br>
        Year: 1978<br>
        ISBN: 089118046X; 9780891180463<br>
        @book{book:{97488242},
           title =     {Biological N fixation in forage-livestock systems},
           author =    {Hoveland, C. S.},
           year =      {1978}}
        </td></tr>
    "#;

    #[test]
    fn reads_the_labelled_lines_off_a_record_page() {
        let d = parse(PAGE, "d40695045e16750abb83015f002af0f4");
        assert_eq!(d.title, "Biological N fixation in forage-livestock systems");
        assert_eq!(d.series, "ASA special publication 28");
        assert_eq!(d.authors, "Hoveland, C. S.;Kral, David M.");
        assert_eq!(d.publisher, "American Society of Agronomy");
        assert_eq!(d.year, "1978");
        assert_eq!(d.isbn, "089118046X; 9780891180463");
        assert_eq!(d.cover_src, "/covers/3342000/d40695045e16750abb83015f002af0f4.jpg");
    }

    /// Mirrors pad the BibTeX block into columns, so the needle cannot include
    /// a single space before the brace — that is what made a download started
    /// from a bare MD5 show the hash as its title.
    #[test]
    fn bibtex_values_survive_padding_around_the_equals() {
        let text = "title =     {Padded Title},\n author = {One Author},";
        assert_eq!(bibtex_field(text, "title").as_deref(), Some("Padded Title"));
        assert_eq!(bibtex_field(text, "author").as_deref(), Some("One Author"));
    }

    #[test]
    fn bibtex_falls_back_only_for_fields_the_page_did_not_state() {
        let page = "@book{x, title = {From BibTeX}, year = {1999}}";
        let d = parse(page, "1b9159991f7fb1b3910c0be9ebf7e595");
        assert_eq!(d.title, "From BibTeX");
        assert_eq!(d.year, "1999");
    }

    #[test]
    fn a_label_word_inside_a_value_does_not_start_a_new_field() {
        let page = "Title: A book about Year: 1066 and all that<br>Year: 1978<br>";
        let d = parse(page, "1b9159991f7fb1b3910c0be9ebf7e595");
        assert_eq!(d.title, "A book about Year: 1066 and all that");
        assert_eq!(d.year, "1978");
    }

    #[test]
    fn missing_fields_stay_empty_rather_than_guessing() {
        let d = parse("<html><body>nothing here</body></html>", "abc");
        assert!(d.title.is_empty());
        assert!(d.publisher.is_empty());
        assert!(d.cover_src.is_empty());
    }
}
