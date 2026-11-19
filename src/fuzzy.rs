//! Small fuzzy matcher for the task and project pickers.

/// Scores how well `query` matches `text`; higher is better, `None` means no
/// match. Every whitespace-separated term in the query has to match, either as
/// a substring (preferred, more so at the start of a word) or as an in-order
/// subsequence of characters.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let text = text.to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .map(|term| term_score(term, &text))
        .sum()
}

fn term_score(term: &str, text: &str) -> Option<i32> {
    let length = term.chars().count() as i32;
    if let Some(pos) = text.find(term) {
        let at_word_start = text[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        return Some(if at_word_start { 100 } else { 60 } + length);
    }

    let mut chars = text.chars();
    let mut gaps = 0;
    for wanted in term.chars() {
        loop {
            match chars.next() {
                Some(c) if c == wanted => break,
                Some(_) => gaps += 1,
                None => return None,
            }
        }
    }
    Some((30 - gaps).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(score("", "Briefing"), Some(0));
        assert_eq!(score("  ", "Briefing"), Some(0));
    }

    #[test]
    fn ranks_word_start_over_substring_over_subsequence() {
        let word_start = score("brief", "Briefing · Website relaunch").unwrap();
        let substring = score("rief", "Briefing").unwrap();
        let subsequence = score("bfg", "Briefing").unwrap();
        assert!(word_start > substring);
        assert!(substring > subsequence);
    }

    #[test]
    fn all_terms_must_match() {
        assert!(score("brief website", "Briefing Website relaunch").is_some());
        assert!(score("brief app", "Briefing Website relaunch").is_none());
        assert!(score("xyz", "Briefing").is_none());
    }

    #[test]
    fn is_case_insensitive_and_unicode_safe() {
        assert!(score("CAFÉ", "Koffie in het café").is_some());
        assert!(score("é", "Été").is_some());
    }
}
