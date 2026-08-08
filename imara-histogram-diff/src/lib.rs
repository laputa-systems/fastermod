#![deny(missing_docs)]

//! A focused histogram diff for line-oriented text.
//!
//! This crate contains the histogram algorithm extracted from `imara-diff`.
//! It intentionally does not include Myers, word diffs, unified diff output,
//! or hunk postprocessing. The returned ranges are enough for a caller to
//! choose its own presentation while keeping the algorithm's retained state small.

use std::ops::Range;
use std::slice;

use intern::Interner;

mod histogram;
mod intern;

/// A contiguous changed range in the before and after line sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Lines removed from the before sequence.
    pub before: Range<usize>,
    /// Lines inserted into the after sequence.
    pub after: Range<usize>,
}

/// The histogram diff between two borrowed text values.
///
/// The diff retains interned line IDs, change flags, and borrowed line text so
/// callers can render hunk context without copying or rescanning the inputs.
pub struct Diff<'a> {
    before: Vec<intern::Token>,
    after: Vec<intern::Token>,
    interner: Interner<'a>,
    removed: Vec<bool>,
    added: Vec<bool>,
}

/// Computes a histogram diff over the lines of `before` and `after`.
///
/// Newline separators remain part of their line. A trailing newline does not
/// create an additional empty line. For highly repetitive inputs that exceed
/// the histogram chain limit, the affected region is conservatively reported
/// as changed; this crate does not include a fallback algorithm.
pub fn diff<'a>(before: &'a str, after: &'a str) -> Diff<'a> {
    let mut interner = Interner::new(estimate_line_count(before) + estimate_line_count(after));
    let before_tokens = intern::lines(before, &mut interner);
    let after_tokens = intern::lines(after, &mut interner);

    let mut removed = vec![false; before_tokens.len()];
    let mut added = vec![false; after_tokens.len()];
    let common_prefix = common_prefix(&before_tokens, &after_tokens);
    let common_postfix = common_postfix(
        &before_tokens[common_prefix..],
        &after_tokens[common_prefix..],
    );
    let before_end = before_tokens.len() - common_postfix;
    let after_end = after_tokens.len() - common_postfix;
    histogram::diff(
        &before_tokens[common_prefix..before_end],
        &after_tokens[common_prefix..after_end],
        &mut removed[common_prefix..before_end],
        &mut added[common_prefix..after_end],
        interner.num_tokens(),
    );
    Diff {
        before: before_tokens,
        after: after_tokens,
        interner,
        removed,
        added,
    }
}

impl<'a> Diff<'a> {
    /// Returns the number of lines in the before text.
    pub fn before_len(&self) -> usize {
        self.before.len()
    }

    /// Returns the number of lines in the after text.
    pub fn after_len(&self) -> usize {
        self.after.len()
    }

    /// Returns a before line including its newline, when present.
    ///
    /// Panics if `index` is outside the before line sequence.
    pub fn before_line(&self, index: usize) -> &'a str {
        self.interner.resolve(self.before[index])
    }

    /// Returns an after line including its newline, when present.
    ///
    /// Panics if `index` is outside the after line sequence.
    pub fn after_line(&self, index: usize) -> &'a str {
        self.interner.resolve(self.after[index])
    }

    /// Returns the number of removed lines.
    pub fn removed_count(&self) -> usize {
        self.removed.iter().filter(|&&changed| changed).count()
    }

    /// Returns the number of added lines.
    pub fn added_count(&self) -> usize {
        self.added.iter().filter(|&&changed| changed).count()
    }

    /// Iterates changed ranges in source order.
    pub fn hunks(&self) -> HunkIter<'_> {
        HunkIter {
            removed: self.removed.iter(),
            added: self.added.iter(),
            before_position: 0,
            after_position: 0,
        }
    }
}

/// Iterator over the changed ranges in a [`Diff`].
pub struct HunkIter<'diff> {
    removed: slice::Iter<'diff, bool>,
    added: slice::Iter<'diff, bool>,
    before_position: usize,
    after_position: usize,
}

impl Iterator for HunkIter<'_> {
    type Item = Hunk;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let removed = self
                .removed
                .by_ref()
                .take_while(|&&changed| changed)
                .count();
            let added = self.added.by_ref().take_while(|&&changed| changed).count();
            if removed != 0 || added != 0 {
                let hunk = Hunk {
                    before: self.before_position..self.before_position + removed,
                    after: self.after_position..self.after_position + added,
                };
                self.before_position += removed + 1;
                self.after_position += added + 1;
                return Some(hunk);
            }
            if self.removed.len() == 0 && self.added.len() == 0 {
                return None;
            }
            self.before_position += 1;
            self.after_position += 1;
        }
    }
}

fn estimate_line_count(text: &str) -> usize {
    let mut remaining = text.as_bytes();
    let mut sample_bytes = 0;
    let mut sample_lines = 0;
    while sample_lines < 20 && !remaining.is_empty() {
        let line_len = memchr::memchr(b'\n', remaining).map_or(remaining.len(), |index| index + 1);
        sample_bytes += line_len;
        remaining = &remaining[line_len..];
        sample_lines += 1;
    }
    if sample_bytes == 0 {
        100
    } else {
        text.len()
            .saturating_mul(sample_lines)
            .checked_div(sample_bytes)
            .unwrap_or(usize::MAX)
            .max(1)
    }
}

fn common_prefix<T: PartialEq>(before: &[T], after: &[T]) -> usize {
    before
        .iter()
        .zip(after)
        .take_while(|(before, after)| before == after)
        .count()
}

fn common_postfix<T: PartialEq>(before: &[T], after: &[T]) -> usize {
    before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take_while(|(before, after)| before == after)
        .count()
}

#[cfg(test)]
mod tests {
    use super::diff;

    #[test]
    fn reports_insertions_and_removals() {
        let inserted = diff("a\nb", "a\nx\nb");
        assert_eq!(
            inserted.hunks().collect::<Vec<_>>(),
            vec![super::Hunk {
                before: 1..1,
                after: 1..2,
            }]
        );

        let removed = diff("a\nx\nb", "a\nb");
        assert_eq!(
            removed.hunks().collect::<Vec<_>>(),
            vec![super::Hunk {
                before: 1..2,
                after: 1..1,
            }]
        );
    }

    #[test]
    fn retains_line_endings_without_creating_trailing_empty_line() {
        let diff = diff("a\n", "a\nb\n");
        assert_eq!(diff.before_len(), 1);
        assert_eq!(diff.after_len(), 2);
        assert_eq!(diff.after_line(1), "b\n");
    }

    #[test]
    fn reports_replacements_and_counts() {
        let diff = diff("a\nb\nc", "a\nx\nc");
        assert_eq!(diff.removed_count(), 1);
        assert_eq!(diff.added_count(), 1);
        assert_eq!(
            diff.hunks().collect::<Vec<_>>(),
            vec![super::Hunk {
                before: 1..2,
                after: 1..2,
            }]
        );
    }

    #[test]
    fn reports_no_hunks_for_identical_text() {
        let diff = diff("a\nb\n", "a\nb\n");
        assert_eq!(diff.removed_count(), 0);
        assert_eq!(diff.added_count(), 0);
        assert!(diff.hunks().next().is_none());
    }
}
