use std::borrow::Cow;

use regex_lite::Regex;

pub(crate) struct Substitution {
    template: String,
    smart_case: bool,
}

impl Substitution {
    pub(crate) fn new(template: String, smart_case: bool) -> Self {
        Self {
            template,
            smart_case,
        }
    }

    pub(crate) fn replacement(&self, captures: &regex_lite::Captures<'_>) -> String {
        let mut replacement = String::new();
        captures.expand(&self.template, &mut replacement);
        if self.smart_case {
            adapt_case(captures.get(0).unwrap().as_str(), &replacement)
        } else {
            replacement
        }
    }

    pub(crate) fn replace_all<'a>(&self, regex: &Regex, contents: &'a str) -> Cow<'a, str> {
        if self.smart_case {
            regex.replace_all(contents, |captures: &regex_lite::Captures<'_>| {
                self.replacement(captures)
            })
        } else {
            regex.replace_all(contents, self.template.as_str())
        }
    }
}

pub(crate) fn adapt_case(matched: &str, replacement: &str) -> String {
    let mut letters = matched
        .chars()
        .filter(|ch| ch.is_lowercase() || ch.is_uppercase());
    let Some(first_letter) = letters.next() else {
        return replacement.to_owned();
    };
    if first_letter.is_uppercase() && letters.clone().all(char::is_uppercase) {
        return replacement.chars().flat_map(char::to_uppercase).collect();
    }
    if first_letter.is_lowercase() && letters.clone().all(char::is_lowercase) {
        return replacement.to_lowercase();
    }
    if first_letter.is_uppercase() && letters.all(char::is_lowercase) {
        let mut first_letter = true;
        let mut title_case = String::new();
        for ch in replacement.chars() {
            if ch.is_uppercase() || ch.is_lowercase() {
                if first_letter {
                    title_case.extend(ch.to_uppercase());
                    first_letter = false;
                } else {
                    title_case.extend(ch.to_lowercase());
                }
            } else {
                title_case.push(ch);
            }
        }
        return title_case;
    }
    replacement.to_owned()
}

pub(crate) fn index_to_row_col(s: &str, index: usize) -> (usize, usize) {
    let chunk = &s[..index];
    let line_num = chunk.chars().filter(|x| x == &'\n').count();
    let last_newline = if let Some(result) = chunk.rfind('\n') {
        result as isize
    } else {
        -1
    };
    let col = index as isize - last_newline - 1;
    (line_num, col as usize)
}

pub(crate) fn resume_after(s: &str, index: usize, zero_length: bool) -> usize {
    let index = index.min(s.len());
    if zero_length {
        one_char_past(s, index)
    } else {
        index
    }
}

pub(crate) fn one_char_past(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        index + s[index..].chars().next().unwrap().len_utf8()
    }
}

pub(crate) fn backward_to_char_boundary(s: &str, mut index: usize) -> usize {
    while !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) fn without_line_ending(line: &str) -> &str {
    if let Some(line) = line.strip_suffix('\n') {
        line.strip_suffix('\r').unwrap_or(line)
    } else {
        line
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_to_row_col() {
        assert_eq!(index_to_row_col("abc", 1), (0, 1));
        assert_eq!(index_to_row_col("abc\ndef", 2), (0, 2));
        assert_eq!(index_to_row_col("abc\ndef", 3), (0, 3));
        assert_eq!(index_to_row_col("abc\ndef", 4), (1, 0));
        assert_eq!(index_to_row_col("abc\ndef\nghi", 8), (2, 0));
    }
}
