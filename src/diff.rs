use std::cmp::max;
use std::cmp::min;

use imara_histogram_diff::Diff as HistogramDiff;
use imara_histogram_diff::diff as histogram_diff;

use crate::text::without_line_ending;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DiffResult<'a> {
    Left(&'a str),
    Both(&'a str, &'a str),
    Right(&'a str),
}


pub(crate) fn compute_diff<'a>(orig: &'a str, edit: &'a str) -> HistogramDiff<'a> {
    histogram_diff(orig, edit)
}

pub(crate) fn diff_output_shape(diff: &HistogramDiff<'_>) -> (usize, usize, usize) {
    let mut output_len = 0;
    let mut before_position = 0;
    let mut after_position = 0;
    let mut prefix_len = None;

    for hunk in diff.hunks() {
        let before_start = hunk.before.start;
        let before_end = hunk.before.end;
        let after_start = hunk.after.start;
        let after_end = hunk.after.end;

        prefix_len.get_or_insert(before_start);
        output_len += before_start - before_position;
        output_len += before_end - before_start;
        output_len += after_end - after_start;
        before_position = before_end;
        after_position = after_end;
    }

    let suffix_len = (diff.before_len() - before_position).min(diff.after_len() - after_position);
    output_len += suffix_len;
    let prefix_len = prefix_len.unwrap_or(output_len);
    (output_len, prefix_len, suffix_len)
}

pub(crate) fn visit_diff_lines<'a>(diff: &HistogramDiff<'a>, mut visit: impl FnMut(DiffResult<'a>) -> bool) {
    let mut before_position = 0;
    let mut after_position = 0;
    for hunk in diff.hunks() {
        let before_start = hunk.before.start;
        let before_end = hunk.before.end;
        let after_start = hunk.after.start;
        let after_end = hunk.after.end;

        debug_assert_eq!(before_start - before_position, after_start - after_position);
        for token in before_position..before_start {
            let before_line = without_line_ending(diff.before_line(token));
            let after_line =
                without_line_ending(diff.after_line(after_position + token - before_position));
            if !visit(DiffResult::Both(before_line, after_line)) {
                return;
            }
        }
        for token in before_start..before_end {
            if !visit(DiffResult::Left(without_line_ending(
                diff.before_line(token),
            ))) {
                return;
            }
        }
        for token in after_start..after_end {
            if !visit(DiffResult::Right(without_line_ending(
                diff.after_line(token),
            ))) {
                return;
            }
        }

        before_position = before_end;
        after_position = after_end;
    }
    for (before_token, after_token) in
        (before_position..diff.before_len()).zip(after_position..diff.after_len())
    {
        if !visit(DiffResult::Both(
            without_line_ending(diff.before_line(before_token)),
            without_line_ending(diff.after_line(after_token)),
        )) {
            return;
        }
    }
}

pub(crate) fn diff_lines_in_range<'a>(
    diff: &HistogramDiff<'a>,
    start: usize,
    end: usize,
) -> Vec<DiffResult<'a>> {
    let mut result = Vec::with_capacity(end - start);
    let mut output_position = 0;
    visit_diff_lines(diff, |line| {
        if output_position >= start {
            result.push(line);
        }
        output_position += 1;
        output_position < end
    });
    result
}

pub(crate) fn diff_line_span(diff: &HistogramDiff<'_>) -> (usize, usize) {
    let mut start = usize::MAX;
    let mut end = 0;
    for hunk in diff.hunks() {
        let hunk_start = min(hunk.before.start, hunk.after.start);
        let hunk_end = max(hunk.before.end, hunk.after.end);
        start = min(start, hunk_start);
        end = max(end, hunk_end);
    }
    if start == usize::MAX {
        return (1, 1);
    }
    (start + 1, max(end, start + 1))
}
