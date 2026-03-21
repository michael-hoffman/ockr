//! Wikilink preprocessor.
//!
//! Rewrites `[[Note Title]]` and `[[Note Title|Display Text]]` syntax in typst
//! source to valid `#link(...)` calls before the document is handed to the
//! compiler. The source file on disk is never modified.
//!
//! ## Normalisation
//!
//! Titles are matched case-insensitively with hyphens and underscores treated
//! as spaces, so `[[Bayes Theorem]]` resolves to `bayes-theorem.typ`.

use std::collections::HashMap;

use crate::vault::VaultFile;

/// Rewrite all `[[wikilinks]]` in `source` to typst `#link(...)` calls.
///
/// - `[[Note Title]]` → `#link("path/to/note.typ")[Note Title]`
/// - `[[Note Title|Display Text]]` → `#link("path/to/note.typ")[Display Text]`
/// - Unresolved links render as red text instead of causing a compile error.
pub fn preprocess_wikilinks(source: &str, files: &[VaultFile]) -> String {
    // Build normalised-title → vault-relative-path lookup.
    let index: HashMap<String, String> = files
        .iter()
        .map(|f| {
            let path = f.rel_path.to_string_lossy().replace('\\', "/");
            (normalise(&f.title), path)
        })
        .collect();

    let mut result = source.to_owned();
    let mut offset = 0usize;

    loop {
        // Find the next `[[` at or after `offset`.
        let Some(rel_open) = result[offset..].find("[[") else { break };
        let open = offset + rel_open;

        // Find the closing `]]` after the opening `[[`.
        let Some(rel_close) = result[open + 2..].find("]]") else { break };
        let close = open + 2 + rel_close;

        let inner = result[open + 2..close].to_owned();

        // Split on the first `|` for an optional display-text override.
        let (target, display) = match inner.find('|') {
            Some(pipe) => (inner[..pipe].trim().to_owned(), inner[pipe + 1..].trim().to_owned()),
            None => (inner.trim().to_owned(), inner.trim().to_owned()),
        };

        let key = normalise(&target);
        let replacement = if let Some(path) = index.get(&key) {
            format!("#link(\"{path}\")[{display}]")
        } else {
            // Broken link: render in coral-red so the user can see it,
            // but do not produce a compile error.
            format!("#text(fill: rgb(\"#ff6e6e\"))[{display}]")
        };

        result.replace_range(open..close + 2, &replacement);
        offset = open + replacement.len();
    }

    result
}

/// Normalise a wikilink title for case-insensitive, whitespace-tolerant matching.
///
/// Lowercase, replace hyphens/underscores with spaces, collapse whitespace.
/// Used by both the wikilink preprocessor and the editor's link-follower so
/// that `[[Bayes Theorem]]`, `[[bayes-theorem]]`, and `[[bayes_theorem]]` all
/// resolve to the same file.
pub fn normalise(s: &str) -> String {
    s.to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(title: &str, rel: &str) -> VaultFile {
        VaultFile {
            title: title.to_string(),
            rel_path: PathBuf::from(rel),
            abs_path: PathBuf::from(rel),
        }
    }

    #[test]
    fn resolves_exact_title() {
        let files = vec![file("bayes-theorem", "zettels/bayes-theorem.typ")];
        let out = preprocess_wikilinks("See [[Bayes Theorem]] for details.", &files);
        assert_eq!(out, "See #link(\"zettels/bayes-theorem.typ\")[Bayes Theorem] for details.");
    }

    #[test]
    fn custom_display_text() {
        let files = vec![file("bayes-theorem", "zettels/bayes-theorem.typ")];
        let out = preprocess_wikilinks("See [[Bayes Theorem|Bayes]] for details.", &files);
        assert_eq!(out, "See #link(\"zettels/bayes-theorem.typ\")[Bayes] for details.");
    }

    #[test]
    fn broken_link_renders_as_red_text() {
        let out = preprocess_wikilinks("See [[Missing Note]] here.", &[]);
        assert!(out.contains("Missing Note"), "display text must be preserved");
        assert!(!out.contains("[["), "wikilink syntax must be removed");
        assert!(out.contains("fill:") || out.contains("text"), "broken link must use #text");
    }

    #[test]
    fn no_links_returns_source_unchanged() {
        let src = "No wikilinks here.";
        assert_eq!(preprocess_wikilinks(src, &[]), src);
    }

    #[test]
    fn multiple_links_in_one_source() {
        let files = vec![
            file("alpha", "alpha.typ"),
            file("beta", "beta.typ"),
        ];
        let out = preprocess_wikilinks("[[Alpha]] and [[Beta]].", &files);
        assert!(out.contains("#link(\"alpha.typ\")[Alpha]"));
        assert!(out.contains("#link(\"beta.typ\")[Beta]"));
    }

    #[test]
    fn hyphen_underscore_normalisation() {
        let files = vec![file("my-note", "my-note.typ")];
        // "My Note" (spaces) should match "my-note" (hyphens) after normalisation.
        let out = preprocess_wikilinks("[[My Note]]", &files);
        assert!(out.contains("#link(\"my-note.typ\")"), "hyphen↔space normalisation failed");
    }
}
