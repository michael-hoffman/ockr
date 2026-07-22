//! Tree-sitter-backed expand/shrink-selection (`Alt-o`/`Alt-i`).
//!
//! Parses fresh on each request rather than maintaining an incrementally
//! updated tree — notes are small enough that a full reparse is
//! unnoticeable, and it sidesteps tree-edit invalidation bookkeeping
//! entirely.

/// The byte range `[start, end)` of the smallest syntax node that contains
/// `[sel_start, sel_end)` and is strictly larger than it — climbs to the
/// parent when the selection already exactly matches a node's range, so
/// repeated calls keep growing outward. `None` if parsing fails, there's no
/// node at that range, or the selection is already the whole document.
pub fn expand_selection(text: &str, sel_start: usize, sel_end: usize) -> Option<(usize, usize)> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&codebook_tree_sitter_typst::LANGUAGE.into()).ok()?;
    let tree = parser.parse(text, None)?;
    let mut node = tree.root_node().descendant_for_byte_range(sel_start, sel_end)?;

    // If the found node's range already exceeds the selection, it's the
    // immediate "smallest containing node" answer. Otherwise the selection
    // already exactly matches this node — climb through parents (skipping
    // any whose range is identical to a descendant's, which single-child
    // wrapper rules produce often) until one is strictly larger.
    if node.start_byte() == sel_start && node.end_byte() == sel_end {
        loop {
            node = node.parent()?;
            if node.start_byte() != sel_start || node.end_byte() != sel_end {
                break;
            }
        }
    }
    Some((node.start_byte(), node.end_byte()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_word_to_enclosing_call_then_further() {
        let text = "#foo(bar, baz)";
        // First expand: a zero-width point inside "bar" (what Normal mode's
        // cursor supplies) selects the smallest enclosing node.
        let (s1, e1) = expand_selection(text, 6, 6).unwrap();
        assert_eq!(&text[s1..e1], "bar");
        // Second expand: the selection now exactly matches that node, so
        // this climbs to its parent — the call's argument list.
        let (s2, e2) = expand_selection(text, s1, e1).unwrap();
        assert_eq!(&text[s2..e2], "(bar, baz)");
    }

    #[test]
    fn no_further_expansion_past_the_root_is_none() {
        let text = "hello";
        // Repeatedly expand until we hit the root, then one more must be None.
        // Bounded so a regression that stops making progress fails fast
        // instead of hanging the test suite.
        let mut range = (0usize, 0usize);
        let mut cur = expand_selection(text, 2, 2);
        for _ in 0..50 {
            let Some((s, e)) = cur else { break };
            range = (s, e);
            cur = expand_selection(text, s, e);
        }
        assert!(cur.is_none(), "expand_selection should terminate at the root within 50 steps");
        assert_eq!(range, (0, text.len()));
    }
}
