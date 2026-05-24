//! Incremental, case-insensitive substring filtering for the popup search box.

use crate::model::Entry;

/// Return the entries matching `query`, preserving input order.
///
/// An empty query returns everything (restoring the full list when the search
/// box is cleared). Matching checks the full text of text entries and the
/// preview of image entries.
pub fn filter<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|e| {
            e.preview.to_lowercase().contains(&q)
                || e.text
                    .as_deref()
                    .is_some_and(|t| t.to_lowercase().contains(&q))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all() {
        let entries = vec![Entry::new_text("a", 1), Entry::new_text("b", 2)];
        assert_eq!(filter(&entries, "").len(), 2);
        assert_eq!(filter(&entries, "   ").len(), 2);
    }

    #[test]
    fn matches_case_insensitive_substring() {
        let entries = vec![
            Entry::new_text("Hello World", 1),
            Entry::new_text("foobar", 2),
        ];
        let r = filter(&entries, "hello");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text.as_deref(), Some("Hello World"));
        assert_eq!(filter(&entries, "BAR").len(), 1);
        assert_eq!(filter(&entries, "zzz").len(), 0);
    }
}
