//! Asynchronous spell checking via `NSSpellChecker`.
//!
//! The synchronous `checkSpellingOfString` API blocks the calling thread on an
//! XPC round-trip whose reply is delivered through the main run loop — calling
//! it *on* the main thread deadlocks (the window never paints).  Instead we use
//! `requestCheckingOfString:…completionHandler:`, which returns immediately and
//! invokes the completion block later on the main run loop.  Results are handed
//! back to the editor through a channel so the render path never blocks.
//!
//! Non-macOS builds compile to a no-op that reports no misspellings.

use futures::channel::mpsc::UnboundedSender;

/// A misspelled span: `(line, byte_start, byte_end)` within the buffer.
pub type SpellSpan = (usize, usize, usize);

/// Result of one spell-check request, tagged with the sequence number it was
/// issued under so the editor can discard stale replies.
pub type SpellResult = (u64, Vec<SpellSpan>);

/// Issue an async spell-check over `text`.  When the language service replies
/// (on the main run loop), the mapped spans are sent on `tx` tagged with `seq`.
///
/// Must be called on the main thread (it touches `NSSpellChecker`).
#[cfg(target_os = "macos")]
pub fn request(text: &str, seq: u64, tx: UnboundedSender<SpellResult>) {
    use block2::RcBlock;
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{
        NSArray, NSInteger, NSOrthography, NSRange, NSString, NSTextCheckingResult,
        NSTextCheckingType,
    };
    use std::ptr::NonNull;

    if text.is_empty() {
        let _ = tx.unbounded_send((seq, Vec::new()));
        return;
    }

    let ns_text = NSString::from_str(text);
    let len_utf16 = ns_text.length(); // NSUInteger, UTF-16 units
    let checker = unsafe { NSSpellChecker::sharedSpellChecker() };

    // Own a copy of the text inside the completion block so offset mapping is
    // self-consistent regardless of later edits.
    let owned = text.to_string();

    let handler = RcBlock::new(
        move |_tag: NSInteger,
              results: NonNull<NSArray<NSTextCheckingResult>>,
              _orth: NonNull<NSOrthography>,
              _count: NSInteger| {
            let results = unsafe { results.as_ref() };
            let mut spans: Vec<SpellSpan> = Vec::new();
            for r in results.iter() {
                let range = r.range();
                if let Some(span) = map_utf16_range(&owned, range.location, range.length) {
                    // Skip 1–2 char "words" (markup/abbreviations), matching the
                    // old behaviour.
                    if span.2 - span.1 > 2 {
                        spans.push(span);
                    }
                }
            }
            let _ = tx.unbounded_send((seq, spans));
        },
    );

    unsafe {
        checker.requestCheckingOfString_range_types_options_inSpellDocumentWithTag_completionHandler(
            &ns_text,
            NSRange::new(0, len_utf16),
            NSTextCheckingType::Spelling.0,
            None,
            0,
            Some(&handler),
        );
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request(_text: &str, seq: u64, tx: UnboundedSender<SpellResult>) {
    let _ = tx.unbounded_send((seq, Vec::new()));
}

/// Map a UTF-16 `[location, location+length)` range over the whole buffer text
/// to a `(line, byte_start, byte_end)` span.  Returns `None` if the range is
/// out of bounds.  Assumes a misspelling does not span a newline.
#[cfg(target_os = "macos")]
fn map_utf16_range(text: &str, location: usize, length: usize) -> Option<SpellSpan> {
    let start_byte = utf16_offset_to_byte(text, location)?;
    let end_byte = utf16_offset_to_byte(text, location + length)?;

    // Locate the line containing start_byte and its starting byte offset.
    let line = text[..start_byte].bytes().filter(|&b| b == b'\n').count();
    let line_start = text[..start_byte]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    Some((line, start_byte - line_start, end_byte - line_start))
}

/// UTF-16 code-unit offset over `text` → byte offset. Returns `None` past end.
#[cfg(target_os = "macos")]
fn utf16_offset_to_byte(text: &str, utf16_off: usize) -> Option<usize> {
    if utf16_off == 0 {
        return Some(0);
    }
    let mut units = 0usize;
    for (b, c) in text.char_indices() {
        if units == utf16_off {
            return Some(b);
        }
        units += c.len_utf16();
        if units > utf16_off {
            return None; // lands mid-codepoint — shouldn't happen for word spans
        }
    }
    if units == utf16_off {
        Some(text.len())
    } else {
        None
    }
}
