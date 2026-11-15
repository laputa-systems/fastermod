//! Typed capability registry: one table drives `--languages` output and
//! engine selection. A row records grammar identity, license, file
//! selection and which operations the language supports. Parser
//! availability is never reported as rename support.

use std::path::Path;
use std::str::FromStr;

use ast_grep_core::matcher::PatternBuilder;
use ast_grep_core::meta_var::MetaVariable;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{Language as AstGrepLanguage, Pattern, PatternError};
use ast_grep_language::SupportLang;
use tree_sitter_language::LanguageFn;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindingSupport {
    /// No binding resolution; syntax-only operations.
    None,
    /// Local binding rename implemented (wave A JS/TS via oxc).
    Local,
    /// Workspace (cross-file) rename implemented (wave A).
    Workspace,
}

pub(crate) struct LangSpec {
    /// Stable language id used in --lang, JSON and diagnostics.
    pub id: &'static str,
    /// File extensions (no dot) that select this language.
    pub extensions: &'static [&'static str],
    /// ast-grep/tree-sitter grammar.
    pub grammar: Grammar,
    /// --ident supported.
    pub ident: bool,
    /// Extra node kinds treated as identifier tokens for --ident
    /// (e.g. Haskell's `variable`).
    pub ident_kinds: &'static [&'static str],
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
/// the bundled tree-sitter grammar revisions. Wave-C languages may add
/// standalone `tree-sitter-*` crates published to the stuff registry.
pub(crate) const GRAMMAR_BUNDLE: &str = "ast-grep-language 0.51.2 + crates.io tree-sitter grammars";

/// A tree-sitter grammar usable by both ast-grep pattern matching and
/// direct tree-sitter traversal. `AstGrep` delegates every language
/// behavior (meta-var chars, pattern preprocessing, kind ids) to the
/// bundled `SupportLang`; `Raw` wraps a standalone `tree-sitter-*`
/// crate's `LANGUAGE` constant for grammars ast-grep-language does not
/// bundle.
#[derive(Clone, Copy)]
pub(crate) enum Grammar {
    AstGrep(SupportLang),
    Raw(LanguageFn),
}

impl Grammar {
    /// The `SupportLang` behind an ast-grep-bundled grammar, if any.
    pub(crate) fn support_lang(&self) -> Option<SupportLang> {
        match self {
            Grammar::AstGrep(l) => Some(*l),
            Grammar::Raw(_) => None,
        }
    }

    /// tree-sitter `Language` handle for direct parser use.
    pub(crate) fn ts_language(&self) -> tree_sitter::Language {
        match self {
            Grammar::AstGrep(l) => l.get_ts_language(),
            Grammar::Raw(f) => (*f).into(),
        }
    }
}

impl AstGrepLanguage for Grammar {
    fn kind_to_id(&self, kind: &str) -> u16 {
        match self {
            Grammar::AstGrep(l) => l.kind_to_id(kind),
            Grammar::Raw(_) => self.ts_language().id_for_node_kind(kind, true),
        }
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        match self {
            Grammar::AstGrep(l) => l.field_to_id(field),
            Grammar::Raw(_) => self
                .ts_language()
                .field_id_for_name(field)
                .map(|id| id.get()),
        }
    }

    fn meta_var_char(&self) -> char {
        match self {
            Grammar::AstGrep(l) => l.meta_var_char(),
            Grammar::Raw(_) => '$',
        }
    }

    fn expando_char(&self) -> char {
        match self {
            Grammar::AstGrep(l) => l.expando_char(),
            Grammar::Raw(_) => '$',
        }
    }

    fn extract_meta_var(&self, source: &str) -> Option<MetaVariable> {
        match self {
            Grammar::AstGrep(l) => l.extract_meta_var(source),
            Grammar::Raw(_) => DollarLang.extract_meta_var(source),
        }
    }

    fn pre_process_pattern<'q>(&self, query: &'q str) -> std::borrow::Cow<'q, str> {
        match self {
            Grammar::AstGrep(l) => l.pre_process_pattern(query),
            Grammar::Raw(_) => std::borrow::Cow::Borrowed(query),
        }
    }

    fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        builder.build(|src| StrDoc::try_new(src, *self))
    }
}

impl LanguageExt for Grammar {
    fn get_ts_language(&self) -> TSLanguage {
        self.ts_language()
    }
}

/// Meta-variable extraction for `Raw` grammars: ast-grep's default
/// implementation only needs `expando_char`, which is `$` for every
/// grammar we wrap. The remaining `Language` methods are unreachable
/// because `DollarLang` is never used to parse or build patterns.
#[derive(Clone)]
struct DollarLang;

impl AstGrepLanguage for DollarLang {
    fn kind_to_id(&self, _kind: &str) -> u16 {
        unreachable!()
    }
    fn field_to_id(&self, _field: &str) -> Option<u16> {
        unreachable!()
    }
    fn build_pattern(&self, _builder: &PatternBuilder) -> Result<Pattern, PatternError> {
        unreachable!()
    }
}


/// Wave A languages. Order is display order.
pub(crate) const LANGUAGES: &[LangSpec] = &[
    LangSpec {
        id: "rust",
        extensions: &["rs"],
        grammar: Grammar::AstGrep(SupportLang::Rust),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Workspace,
        ident_kinds: &[],
        license: "MIT (tree-sitter-rust)",
        limitations: "identifiers inside macro invocations are matched syntactically only; lifetime and label names are not identifier tokens",
        fixtures: &["ident-rust", "rewrite-rust", "rename-rust"],
    },
    LangSpec {
        id: "python",
        extensions: &["py", "pyi"],
        grammar: Grammar::AstGrep(SupportLang::Python),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Workspace,
        ident_kinds: &[],
        license: "MIT (tree-sitter-python)",
        limitations: "dynamic attribute access and __getattr__ targets are invisible to syntax matching",
        fixtures: &["ident-python", "rewrite-python", "rename-python"],
    },
    LangSpec {
        id: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
        grammar: Grammar::AstGrep(SupportLang::JavaScript),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Workspace,
        ident_kinds: &[],
        license: "MIT (tree-sitter-javascript)",
        limitations: "JSX in .js files parses only when unambiguous; dynamic property access obj[name] is not a rename site",
        fixtures: &["ident-javascript", "rewrite-javascript", "rename-javascript"],
    },
    LangSpec {
        id: "typescript",
        extensions: &["ts", "cts", "mts"],
        grammar: Grammar::AstGrep(SupportLang::TypeScript),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Workspace,
        ident_kinds: &[],
        license: "MIT (tree-sitter-typescript)",
        limitations: "type positions and value positions share the identifier token rule; JSX is not parsed in .ts files",
        fixtures: &["ident-typescript", "rewrite-typescript", "rename-typescript"],
    },
    LangSpec {
        id: "tsx",
        extensions: &["tsx"],
        grammar: Grammar::AstGrep(SupportLang::Tsx),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Workspace,
        ident_kinds: &[],
        license: "MIT (tree-sitter-typescript)",
        limitations: "same grammar as typescript with JSX enabled; angle-bracket type assertions are ambiguous with JSX and are not supported",
        fixtures: &["ident-tsx", "rewrite-tsx", "rename-typescript"],
    },
    // ---- wave B (REFAC-9): syntax operations shipped; binding rows
    // land per-language as analyzers are implemented. ----
    LangSpec {
        id: "go",
        extensions: &["go"],
        grammar: Grammar::AstGrep(SupportLang::Go),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-go)",
        limitations: "method names, struct/interface field names and selector fields are separate namespaces and are not rename sites; cross-file package rename is not supported",
        fixtures: &["ident-go", "rewrite-go", "rename-go"],
    },
    LangSpec {
        id: "c",
        extensions: &["c", "h"],
        grammar: Grammar::AstGrep(SupportLang::C),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-c)",
        limitations: ".h files are analyzed as C; preprocessor directives are not semantic sites; struct/union member access (s.x, p->x) blocks member renames; cross-file resolution is not modeled",
        fixtures: &["ident-c", "rewrite-c", "rename-c"],
    },
    LangSpec {
        id: "cpp",
        extensions: &["cc", "cpp", "cxx", "hpp", "hh", "h++", "cu", "ino"],
        grammar: Grammar::AstGrep(SupportLang::Cpp),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-cpp)",
        limitations: "preprocessor directives and template instantiations are matched syntactically only; member access on non-this receivers (o.m, p->x) blocks member renames; cross-file/header resolution is not modeled",
        fixtures: &["ident-cpp", "rewrite-cpp", "rename-cpp"],
    },
    LangSpec {
        id: "java",
        extensions: &["java"],
        grammar: Grammar::AstGrep(SupportLang::Java),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-java)",
        limitations: "qualified member access obj.m()/obj.f cannot be resolved and blocks member renames; imports are not resolved so cross-file type/member renames block on unresolved occurrences",
        fixtures: &["ident-java", "rewrite-java", "rename-java"],
    },
    LangSpec {
        id: "csharp",
        extensions: &["cs"],
        grammar: Grammar::AstGrep(SupportLang::CSharp),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-c-sharp)",
        limitations: "obj.M()/obj.f on non-this receivers blocks member renames; using/namespace imports unresolved so cross-file renames block; extension methods and dynamic dispatch unsupported",
        fixtures: &["ident-csharp", "rewrite-csharp", "rename-csharp"],
    },
    LangSpec {
        id: "kotlin",
        extensions: &["kt", "kts"],
        grammar: Grammar::AstGrep(SupportLang::Kotlin),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-kotlin)",
        limitations: "navigation a.b on non-this receivers blocks member renames; imports, extension functions, delegated properties and implicit `it` are not resolved",
        fixtures: &["ident-kotlin", "rewrite-kotlin", "rename-kotlin"],
    },
    LangSpec {
        id: "swift",
        extensions: &["swift"],
        grammar: Grammar::AstGrep(SupportLang::Swift),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-swift)",
        limitations: "obj.m/obj.p on non-self receivers blocks member renames; closures, property wrappers, result builders and operators are only partially modeled",
        fixtures: &["ident-swift", "rewrite-swift", "rename-swift"],
    },
    LangSpec {
        id: "php",
        extensions: &["php", "phtml"],
        grammar: Grammar::AstGrep(SupportLang::Php),
        ident: false,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-php)",
        limitations: "variable variables ($$x), extract/compact and dynamic calls are invisible; member access on non-$this receivers blocks; require/include resolution unsupported",
        fixtures: &["rewrite-php", "rename-php"],
    },
    LangSpec {
        id: "ruby",
        extensions: &["rb"],
        grammar: Grammar::AstGrep(SupportLang::Ruby),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-ruby)",
        limitations: "metaprogramming (eval, method_missing, define_method, attr_accessor-generated methods) is invisible; obj.m() on unknown receivers blocks; require/include resolution unsupported",
        fixtures: &["ident-ruby", "rewrite-ruby", "rename-ruby"],
    },
    LangSpec {
        id: "dart",
        extensions: &["dart"],
        grammar: Grammar::AstGrep(SupportLang::Dart),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-dart)",
        limitations: "o.m/o.f on non-this receivers blocks member renames; import resolution, noSuchMethod and dynamic dispatch are not modeled",
        fixtures: &["ident-dart", "rewrite-dart", "rename-dart"],
    },
    LangSpec {
        id: "lua",
        extensions: &["lua"],
        grammar: Grammar::AstGrep(SupportLang::Lua),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-lua)",
        limitations: "dynamic global resolution (_ENV, load, setfenv, rawset on _G) is invisible; unresolved same-name occurrences block rename",
        fixtures: &["ident-lua", "rewrite-lua", "rename-lua"],
    },
    LangSpec {
        id: "bash",
        extensions: &["sh", "bash"],
        grammar: Grammar::AstGrep(SupportLang::Bash),
        ident: false,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-bash)",
        limitations: "posix shell subset; aliases, eval, indirect expansion and namerefs are not modeled; no block scoping (matches shell semantics)",
        fixtures: &["rewrite-bash", "rename-bash"],
    },
    LangSpec {
        id: "scala",
        extensions: &["scala", "sc"],
        grammar: Grammar::AstGrep(SupportLang::Scala),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-scala)",
        limitations: "obj.m/Type.m on non-this receivers blocks member renames; pattern bindings, givens, implicits and macros are only partially modeled; file-local only",
        fixtures: &["ident-scala", "rewrite-scala", "rename-scala"],
    },
    LangSpec {
        id: "elixir",
        extensions: &["ex", "exs"],
        grammar: Grammar::AstGrep(SupportLang::Elixir),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-elixir)",
        limitations: "Mod.fun remote calls record the member unresolved; macros, apply/send, sigils and unquote fragments are only partially modeled; file-local only",
        fixtures: &["ident-elixir", "rewrite-elixir", "rename-elixir"],
    },
    LangSpec {
        id: "haskell",
        extensions: &["hs", "lhs"],
        grammar: Grammar::AstGrep(SupportLang::Haskell),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &["variable"],
        license: "MIT (tree-sitter-haskell)",
        limitations: "identifier tokens use the `variable` node kind; qualified names, record fields, do-let scoping granularity and TH are only partially modeled; file-local only",
        fixtures: &["ident-haskell", "rewrite-haskell", "rename-haskell"],
    },
    LangSpec {
        id: "zig",
        extensions: &["zig"],
        grammar: Grammar::AstGrep(SupportLang::Zig),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-zig)",
        limitations: "obj.member/ptr->member access blocks member renames; @import namespacing, comptime and anytype dispatch are not modeled; file-local only",
        fixtures: &["ident-zig", "rewrite-zig", "rename-zig"],
    },
    // Wave C: standalone tree-sitter grammar crates (published to the
    // stuff crates registry). binding stays None until per-language
    // analyzers land; ident/rewrite are verified by fixtures.
    LangSpec {
        id: "r",
        extensions: &["r", "R"],
        grammar: Grammar::Raw(tree_sitter_r::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-r)",
        limitations: "S3/R6 method dispatch, Recall/eval tricks and non-standard evaluation are not modeled; pkg::fn blocks; file-local only",
        fixtures: &["ident-r", "rewrite-r", "rename-r"],
    },
    LangSpec {
        id: "julia",
        extensions: &["jl"],
        grammar: Grammar::Raw(tree_sitter_julia::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-julia)",
        limitations: "multiple dispatch and generated functions are not modeled; qualified calls obj.f() are not resolved; file-local only",
        fixtures: &["ident-julia", "rewrite-julia", "rename-julia"],
    },
    LangSpec {
        id: "objective-c",
        extensions: &["m", "mm"],
        grammar: Grammar::Raw(tree_sitter_objc::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-objc)",
        limitations: ".m files also match MATLAB/Octave; this row wins by registry order; Objective-C message dispatch is not modeled",
        fixtures: &["ident-objective-c", "rewrite-objective-c"],
    },
    LangSpec {
        id: "erlang",
        extensions: &["erl", "hrl"],
        grammar: Grammar::Raw(tree_sitter_erlang::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &["atom", "var"],
        license: "MIT (tree-sitter-erlang)",
        limitations: "records, maps and process dictionary semantics are not modeled; atoms are globally interned so literal atoms sharing a function name are renamed too; file-local only",
        fixtures: &["ident-erlang", "rewrite-erlang", "rename-erlang"],
    },
    LangSpec {
        id: "ocaml",
        extensions: &["ml"],
        grammar: Grammar::Raw(tree_sitter_ocaml::LANGUAGE_OCAML),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &["value_name", "type_constructor", "constructor_name"],
        license: "MIT (tree-sitter-ocaml)",
        limitations: "module system, functors and GADTs are only partially modeled",
        fixtures: &["ident-ocaml", "rewrite-ocaml", "rename-ocaml"],
    },
    LangSpec {
        id: "ocaml-interface",
        extensions: &["mli"],
        grammar: Grammar::Raw(tree_sitter_ocaml::LANGUAGE_OCAML_INTERFACE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &["value_name", "type_constructor", "constructor_name"],
        license: "MIT (tree-sitter-ocaml)",
        limitations: "signature files only; implementation files use the ocaml row",
        fixtures: &["ident-ocaml-interface", "rewrite-ocaml-interface"],
    },
    LangSpec {
        id: "fsharp",
        extensions: &["fs", "fsx"],
        grammar: Grammar::Raw(tree_sitter_fsharp::LANGUAGE_FSHARP),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-fsharp)",
        limitations: ".fsi signature files use the fsharp-signature row; computation expressions and units of measure are not modeled",
        fixtures: &["ident-fsharp", "rewrite-fsharp", "rename-fsharp"],
    },
    LangSpec {
        id: "fsharp-signature",
        extensions: &["fsi"],
        grammar: Grammar::Raw(tree_sitter_fsharp::LANGUAGE_SIGNATURE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-fsharp)",
        limitations: "signature files only",
        fixtures: &["ident-fsharp-signature", "rewrite-fsharp-signature"],
    },
    LangSpec {
        id: "groovy",
        extensions: &["groovy", "gradle"],
        grammar: Grammar::Raw(tree_sitter_groovy::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-groovy)",
        limitations: "tree-sitter-groovy 0.1.2 is immature: some method-body expressions fail to parse (files are skipped); Gradle DSL dynamic dispatch is not modeled",
        fixtures: &["ident-groovy", "rewrite-groovy", "rename-groovy"],
    },
    LangSpec {
        id: "fortran",
        extensions: &["f90", "f95", "f03", "f08"],
        grammar: Grammar::Raw(tree_sitter_fortran::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &["name"],
        license: "MIT (tree-sitter-fortran)",
        limitations: "fixed-form and F77 are not supported; Fortran semantics (argument aliasing, sequence association) are not modeled",
        fixtures: &["ident-fortran", "rewrite-fortran", "rename-fortran"],
    },
    LangSpec {
        id: "matlab",
        extensions: &["m"],
        grammar: Grammar::Raw(tree_sitter_matlab::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-matlab)",
        limitations: ".m files are claimed by objective-c first; select with --lang matlab; Octave dialect and function-handle dispatch are not modeled",
        fixtures: &["ident-matlab", "rewrite-matlab"],
    },
    LangSpec {
        id: "vb-dotnet",
        extensions: &["vb"],
        grammar: Grammar::Raw(tree_sitter_vb_dotnet::LANGUAGE),
        ident: true,
        rewrite: true,
        binding: BindingSupport::Local,
        ident_kinds: &[],
        license: "MIT (tree-sitter-vb-dotnet)",
        limitations: "VB is case-insensitive; rename preserves spelling case per occurrence",
        fixtures: &["ident-vb-dotnet", "rewrite-vb-dotnet", "rename-vb-dotnet"],
    },
    // REFAC-11 structured assets: bundled data/markup grammars.
    // ident stays off for data formats — keys are data, not lexical
    // variables. binding stays None pending asset-specific analyzers.
    LangSpec {
        id: "css",
        extensions: &["css", "scss"],
        grammar: Grammar::AstGrep(SupportLang::Css),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &["class_name", "id_name", "property_name", "type_selector"],
        license: "MIT (tree-sitter-css)",
        limitations: "scss nesting/mixins parse as css subset; @import url resolution unsupported",
        fixtures: &["ident-css", "rewrite-css"],
    },
    LangSpec {
        id: "html",
        extensions: &["html", "htm", "xhtml"],
        grammar: Grammar::AstGrep(SupportLang::Html),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &["tag_name", "attribute_name", "property_name"],
        license: "MIT (tree-sitter-html)",
        limitations: "embedded script/style regions are not edited through host bindings; custom elements and template slots are syntax-only",
        fixtures: &["ident-html", "rewrite-html"],
    },
    LangSpec {
        id: "json",
        extensions: &["json", "jsonc", "json5"],
        grammar: Grammar::AstGrep(SupportLang::Json),
        ident: false,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-json)",
        limitations: "data keys are not lexical variables; JSONC/JSON5 comments tolerated by grammar",
        fixtures: &["rewrite-json"],
    },
    LangSpec {
        id: "yaml",
        extensions: &["yaml", "yml"],
        grammar: Grammar::AstGrep(SupportLang::Yaml),
        ident: false,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-yaml)",
        limitations: "anchors/aliases are not yet modeled as bindings; data keys are not lexical variables",
        fixtures: &["rewrite-yaml"],
    },
    LangSpec {
        id: "hcl",
        extensions: &["hcl", "tf", "tfvars", "nomad", "workflow"],
        grammar: Grammar::AstGrep(SupportLang::Hcl),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-hcl)",
        limitations: "terraform/nomad provider references are unresolved names; file-local syntax only",
        fixtures: &["ident-hcl", "rewrite-hcl"],
    },
    LangSpec {
        id: "nix",
        extensions: &["nix"],
        grammar: Grammar::AstGrep(SupportLang::Nix),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-nix)",
        limitations: "nix scoping (let/inherit/with) is not modeled for rename",
        fixtures: &["ident-nix", "rewrite-nix"],
    },
    LangSpec {
        id: "markdown",
        extensions: &["md", "markdown"],
        grammar: Grammar::AstGrep(SupportLang::Markdown),
        ident: false,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-markdown)",
        limitations: "fenced code blocks are not edited through host-language bindings",
        fixtures: &["rewrite-markdown"],
    },
    LangSpec {
        id: "solidity",
        extensions: &["sol"],
        grammar: Grammar::AstGrep(SupportLang::Solidity),
        ident: true,
        rewrite: true,
        binding: BindingSupport::None,
        ident_kinds: &[],
        license: "MIT (tree-sitter-solidity)",
        limitations: "contract inheritance and modifier dispatch are not modeled for rename",
        fixtures: &["ident-solidity", "rewrite-solidity"],
    },
];

/// Resolve a --lang value (id, alias or SupportLang name) to a spec.
pub(crate) fn by_name(name: &str) -> Option<&'static LangSpec> {
    LANGUAGES.iter().find(|spec| {
        spec.id == name
            || spec.extensions.contains(&name)
            || SupportLang::from_str(name).is_ok_and(|l| spec.grammar.support_lang() == Some(l))
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





