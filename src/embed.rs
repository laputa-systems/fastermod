//! Embedded-language region extraction for host files whose grammars do
//! not parse embedded source (HTML `<script>`/`<style>`, Vue/Svelte SFC
//! blocks, Markdown fenced code blocks). Regions are byte ranges inside
//! the host file; edits are computed on the substring and mapped back by
//! offset, so host bytes and delimiters are preserved exactly.
//!
//! This is a syntactic host mapping only: it does not infer cross-language
//! identity, and `--rename` is not supported inside embedded regions
//! (binding analysis is per-file; region plumbing is REFAC-11 open work).

use std::ops::Range;
use std::path::Path;

use crate::lang::{self, LangSpec};

/// One embedded code region: byte range in the host file plus the
/// language spec that should parse the substring.
pub(crate) struct Region {
    pub range: Range<usize>,
    pub spec: &'static LangSpec,
}

fn spec_for_tag(tag: &str) -> Option<&'static LangSpec> {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    lang::by_name(&tag)
}

/// Extract `attr="value"` / `attr='value'` from a tag's attribute text.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let lower = attrs.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find(name) {
        let i = pos + rel;
        let before_ok = i == 0
            || !(lower.as_bytes()[i - 1].is_ascii_alphanumeric()
                || lower.as_bytes()[i - 1] == b'-');
        let after = i + name.len();
        if before_ok && lower[after..].trim_start().starts_with('=') {
            let eq = after + lower[after..].find('=').unwrap();
            let rest_full = attrs[eq + 1..].trim_start();
            let quote = rest_full.chars().next()?;
            if quote == '"' || quote == '\'' {
                let body = &rest_full[quote.len_utf8()..];
                let end = body.find(quote)?;
                return Some(&body[..end]);
            }
            let end = rest_full
                .find(|c: char| c.is_whitespace() || c == '>')
                .unwrap_or(rest_full.len());
            return Some(&rest_full[..end]);
        }
        pos = after;
    }
    None
}

/// Scan for `<tag ...>...</tag>` blocks (case-insensitive). Returns
/// (content range, tag name, attribute text) in byte order.
fn scan_tag_blocks(contents: &str, tags: &[&str]) -> Vec<(Range<usize>, String, String)> {
    let lower = contents.to_ascii_lowercase();
    let bytes = contents.as_bytes();
    let mut out = Vec::new();
    for tag in tags {
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        let mut pos = 0;
        while let Some(rel) = lower[pos..].find(&open) {
            let start = pos + rel;
            let after = start + open.len();
            if after < bytes.len()
                && !matches!(bytes[after], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')
            {
                pos = after;
                continue;
            }
            let Some(gt_rel) = lower[after..].find('>') else {
                break;
            };
            let gt = after + gt_rel;
            let attrs = contents[after..gt].to_string();
            if attrs.trim_end().ends_with('/') {
                pos = gt + 1;
                continue; // self-closing
            }
            let Some(c_rel) = lower[gt + 1..].find(&close) else {
                pos = gt + 1;
                continue;
            };
            let close_start = gt + 1 + c_rel;
            out.push((gt + 1..close_start, tag.to_string(), attrs));
            pos = close_start + close.len();
        }
    }
    out.sort_by_key(|(r, _, _)| r.start);
    out
}

fn inner_spec_for_block(tag: &str, attrs: &str) -> Option<&'static LangSpec> {
    if let Some(lang) = attr(attrs, "lang") {
        return spec_for_tag(lang);
    }
    if tag == "script" {
        if let Some(ty) = attr(attrs, "type") {
            let ty = ty.to_ascii_lowercase();
            if ty.contains("typescript") {
                return spec_for_tag("ts");
            }
            if ty.starts_with("text/") || ty == "module" {
                return spec_for_tag("js");
            }
            // Unknown script type (e.g. application/json) — not code.
            return None;
        }
        return spec_for_tag("js");
    }
    spec_for_tag("css")
}

/// Markdown fenced code blocks: ```lang or ~~~lang info strings, content
/// until the matching closing fence. Returns (content range, info tag).
fn scan_fences(contents: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut lines = contents.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_end_matches(['\n', '\r']).trim_start();
        if !(t.starts_with("```") || t.starts_with("~~~")) {
            offset += line.len();
            continue;
        }
        let fence = &t[..3];
        let tag = t[3..].trim().to_string();
        let content_start = offset + line.len();
        let mut end = contents.len();
        let mut scanned = content_start;
        for l2 in lines.by_ref() {
            let t2 = l2.trim_end_matches(['\n', '\r']).trim_start();
            scanned += l2.len();
            if t2.starts_with(fence) && t2[fence.len()..].trim().is_empty() {
                end = scanned - l2.len();
                break;
            }
        }
        out.push((content_start..end, tag));
        offset = scanned;
    }
    out
}

/// Embedded regions for `path`/`contents`, in byte order. Returns an
/// empty vec for non-host files.
pub(crate) fn regions(path: &Path, contents: &str) -> Vec<Region> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut raw: Vec<(Range<usize>, &'static LangSpec)> = Vec::new();
    match ext {
        "html" | "htm" | "xhtml" | "vue" | "svelte" => {
            for (range, tag, attrs) in scan_tag_blocks(contents, &["script", "style"]) {
                if let Some(spec) = inner_spec_for_block(&tag, &attrs) {
                    raw.push((range, spec));
                }
            }
        }
        "md" | "markdown" | "mdx" => {
            for (range, tag) in scan_fences(contents) {
                if let Some(spec) = spec_for_tag(&tag) {
                    raw.push((range, spec));
                }
            }
        }
        _ => {}
    }
    raw.sort_by_key(|(r, _)| r.start);
    raw.into_iter()
        .map(|(range, spec)| Region { range, spec })
        .collect()
}
