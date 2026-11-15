//! Typed capability registry: one table drives `--languages` output and
//! engine selection. A row records grammar identity, license, file
//! selection and which operations the language supports. Parser
//! availability is never reported as rename support.

use std::path::Path;
use std::str::FromStr;

use ast_grep_language::SupportLang;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingSupport {
    /// No binding resolution; syntax-only operations.
    None,
    /// Local binding rename implemented (wave A JS/TS via oxc).
    Local,
}

pub(crate) struct LangSpec {
    /// Stable language id used in --lang, JSON and diagnostics.
    pub id: &'static str,
    /// File extensions (no dot) that select this language.
    pub extensions: &'static [&'static str],
    /// ast-grep/tree-sitter grammar.
    pub lang: SupportLang,
    /// --ident supported.
    pub ident: bool,
    /// --rewrite supported.
    pub rewrite: bool,
    /// Binding rename support level.
    pub binding: BindingSupport,
    /// Grammar/license provenance recorded for the capability report.
    pub license: &'static str,
    /// Known limitations reported in --languages (empty string = none).
    pub limitations: &'static str,
    /// Fixture ids covering this language (tests/fixtures/).
    pub fixtures: &'static [&'static str],
}

/// Grammar bundle version recorded in capability output. Keep in sync with
/// the `ast-grep-language` dependency in Cargo.toml — it is the source of
/// the bundled tree-sitter grammar revisions.
pub(crate) const GRAMMAR_BUNDLE: &str = "ast-grep-language 0.51.2";

/// Wave A languages. Order is display order.
pub(crate) const LANGUAGES: &[LangSpec] = &[
    LangSpec {
        id: "rust",
        extensions: &["rs"],
        lang: SupportLang::Rust,
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        license: "MIT (tree-sitter-rust)",
        limitations: "identifiers inside macro invocations are matched syntactically only; lifetime and label names are not identifier tokens",
        fixtures: &["ident-rust", "rewrite-rust"],
    },
    LangSpec {
        id: "python",
        extensions: &["py", "pyi"],
        lang: SupportLang::Python,
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        license: "MIT (tree-sitter-python)",
        limitations: "dynamic attribute access and __getattr__ targets are invisible to syntax matching",
        fixtures: &["ident-python", "rewrite-python"],
    },
    LangSpec {
        id: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
        lang: SupportLang::JavaScript,
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        license: "MIT (tree-sitter-javascript)",
        limitations: "JSX in .js files parses only when unambiguous; dynamic property access obj[name] is not a rename site",
        fixtures: &["ident-javascript", "rewrite-javascript"],
    },
    LangSpec {
        id: "typescript",
        extensions: &["ts", "cts", "mts"],
        lang: SupportLang::TypeScript,
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        license: "MIT (tree-sitter-typescript)",
        limitations: "type positions and value positions share the identifier token rule; JSX is not parsed in .ts files",
        fixtures: &["ident-typescript", "rewrite-typescript"],
    },
    LangSpec {
        id: "tsx",
        extensions: &["tsx"],
        lang: SupportLang::Tsx,
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        license: "MIT (tree-sitter-typescript)",
        limitations: "same grammar as typescript with JSX enabled; angle-bracket type assertions are ambiguous with JSX and are not supported",
        fixtures: &["ident-tsx", "rewrite-tsx"],
    },
];

/// Resolve a --lang value (id, alias or SupportLang name) to a spec.
pub(crate) fn by_name(name: &str) -> Option<&'static LangSpec> {
    LANGUAGES.iter().find(|spec| {
        spec.id == name
            || spec.extensions.contains(&name)
            || SupportLang::from_str(name).is_ok_and(|l| l == spec.lang)
    })
}

/// Detect the language for a path: --lang overrides win, then extension.
pub(crate) fn detect(path: &Path, overrides: &[String]) -> Option<&'static LangSpec> {
    if let Some(name) = overrides.first()
        && let Some(spec) = by_name(name)
    {
        return Some(spec);
    }
    let ext = path.extension()?.to_str()?;
    LANGUAGES
        .iter()
        .find(|spec| spec.extensions.contains(&ext))
}

/// Identifier-token test shared by --ident: a node is an identifier token
/// when its grammar kind is `identifier` or ends in `_identifier`
/// (covers identifier, type_identifier, field_identifier,
/// property_identifier, private_property_identifier, ...).
pub(crate) fn is_identifier_kind(kind: &str) -> bool {
    kind == "identifier" || kind.ends_with("_identifier")
}
