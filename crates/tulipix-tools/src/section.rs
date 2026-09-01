//! `np.p4.tools.section` — Tools sidebar entry + landing page.
//!
//! The taxonomy itself lives in [`crate::catalog`]; this module is the landing
//! page's view of it. It used to hold a second `category_of` in kebab-case that
//! had drifted from the bridge's — `resize` under Photo where the bridge said
//! Video, a `pdf` op the bridge had never heard of — so it is a re-export now.

pub use crate::catalog::Category;

/// Category for an operation kind. Accepts the kebab spelling the CLI uses.
pub fn category_of(op: &str) -> Option<Category> {
    crate::catalog::resolve(op)
        .and_then(crate::catalog::get)
        .map(|o| o.cat)
}

/// Op kinds shown under a category on the landing page, in catalogue order.
pub fn ops_in(category: Category) -> Vec<&'static str> {
    crate::catalog::in_category(category).map(|o| o.kind).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorization() {
        assert_eq!(category_of("rename"), Some(Category::FileOps));
        assert_eq!(category_of("transcribe"), Some(Category::Subtitles));
        assert_eq!(category_of("compress_photo"), Some(Category::Photo));
        // The CLI's spelling resolves to the same op.
        assert_eq!(category_of("compress-photo"), Some(Category::Photo));
        assert_eq!(category_of("frobnicate"), None);
        assert!(ops_in(Category::Audio).contains(&"normalize"));
        assert_eq!(Category::ALL.len(), 7);
    }

    #[test]
    fn one_taxonomy_not_two() {
        // `resize` was Video in the bridge and Photo here. The bridge rendered
        // the grid, so the bridge was right; this asserts they cannot diverge
        // again because there is only one answer now.
        assert_eq!(category_of("resize"), Some(Category::Video));
    }
}
