//! Binding model shared by every language resolver. An analysis is built
//! fresh per invocation from in-memory source; nothing persists.
//!
//! A [`BindingSite`] is a declaration (the identifier token that introduces
//! a name). A [`RefSite`] is an identifier occurrence that resolves to a
//! binding, or `target: None` when resolution failed — unresolved
//! occurrences of the renamed spelling block the rename.
//!
//! Some identifier tokens are syntactically coupled to a second meaning —
//! object/destructuring shorthand, import specifiers without `as`, export
//! specifiers without `as`. [`Expansion`] records the rewrite template that
//! preserves the coupled name instead of a plain token swap.

use std::ops::Range;

/// Rewrite template for identifier tokens that carry a second meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expansion {
    /// `{ OLD }` shorthand (object literal value, destructuring pattern,
    /// field init): becomes `{ OLD: NEW }` — the key keeps the old name.
    ShorthandColon,
    /// Import without alias (`import { OLD }`, `use a::OLD`,
    /// `from m import OLD`): becomes `OLD as NEW` — the imported name is
    /// preserved while the local binding takes the new name.
    ImportAlias,
    /// Export without alias (`export { OLD }`): becomes `NEW as OLD` — the
    /// local binding is renamed while the exported name is preserved.
    ExportAlias,
}

#[derive(Clone, Debug)]
pub(crate) struct BindingSite {
    /// Byte range of the declaring identifier token.
    pub name: Range<usize>,
    /// The identifier text.
    pub text: String,
    /// Extra rewrite needed when the declaration doubles as another name.
    pub expansion: Option<Expansion>,
    /// `true` for bindings introduced at module top level — the only
    /// bindings another file can name through an import.
    pub module_level: bool,
    /// `true` for `mod name;`/`mod name {}` declarations — renaming one
    /// requires renaming the module file, which is out of scope.
    pub is_mod: bool,
}

/// An import edge: this file refers to `name` inside the module denoted by
/// `specifier`. `name_range` is the token carrying the imported spelling
/// (for unaliased imports it coincides with the local binding's name).
/// `local` is the binding index the edge creates, when it creates one.
/// `star` marks `import *`/`export *`/glob `use` — opaque membership.
#[derive(Clone, Debug)]
pub(crate) struct ImportEdge {
    /// Raw specifier text (`./mod`, `pkg.sub`, `crate::a::b`).
    pub specifier: String,
    /// Imported symbol name; empty for module-object binds (`import m`).
    pub name: String,
    /// Byte range of the token spelling `name` (or the local token for
    /// default imports, where no imported-name token exists).
    pub name_range: Range<usize>,
    /// Local binding created by this edge, if any.
    pub local: Option<usize>,
    /// `true` when the edge re-exports (`pub use`, `export ... from`).
    pub reexport: bool,
    /// `true` for wildcard edges (`*`, `use a::*`, dynamic `import()`).
    pub star: bool,
    /// For re-export edges: the spelling this module exports, when it
    /// differs from the module-side `name` (`export { x as y } from`,
    /// `pub use a::x as y`, `export * as ns from`). `None` = same as
    /// `name` (or not a re-export).
    pub exported: Option<String>,
}

/// An export alias declared in this module without a module specifier:
/// `export { local as exported }`, `export default <local>`.
#[derive(Clone, Debug)]
pub(crate) struct ExportName {
    /// The spelling importers use.
    pub exported: String,
    /// Local binding the export refers to, when it names one.
    pub local: Option<usize>,
}

/// A `mod name;` declaration whose body lives in another file. The
/// workspace layer resolves `name` to a sibling source file.
#[derive(Clone, Debug)]
pub(crate) struct ModDecl {
    /// The declared module name.
    pub name: String,
    /// Byte range of the name token (for diagnostics/anchors).
    pub name_range: Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct RefSite {
    /// Byte range of the referencing identifier token.
    pub name: Range<usize>,
    /// Index into `Analysis::bindings`, or `None` when unresolved.
    pub target: Option<usize>,
    /// Extra rewrite needed when the reference sits in a coupled position.
    pub expansion: Option<Expansion>,
    /// `true` for path segments inside another module's namespace
    /// (`a::b::C` tails, `use` path tails). They can never refer to a
    /// lexical local binding, so unresolved remote refs do not block a
    /// rename — but resolved ones still participate in edits.
    pub remote: bool,
    /// `Some(i)` when this occurrence is `module_attr` spelled through a
    /// module-object binding listed in `Analysis::imports[i]` — e.g.
    /// `m.foo` where `m` is `import m` / `import * as m` / `use m`. The
    /// workspace layer resolves the attribute against the target module.
    pub module_attr: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Analysis {
    pub bindings: Vec<BindingSite>,
    pub refs: Vec<RefSite>,
    /// Import edges discovered in this file (named, aliased, wildcard,
    /// re-export and module-object forms).
    pub imports: Vec<ImportEdge>,
    /// Local export aliases (`export { x as y }`, `export default`).
    pub exports: Vec<ExportName>,
    /// `mod name;` declarations pending cross-file resolution.
    pub mod_decls: Vec<ModDecl>,
    /// `true` when the analyzer's unresolved set is provably complete
    /// (oxc resolves every identifier reference it models, so an
    /// unresolved occurrence can never refer to the selected binding).
    /// Hand-rolled CST resolvers set `false`: occurrences they cannot
    /// classify may hide relevant references and must block the rename.
    pub unresolved_is_exact: bool,
}

impl Analysis {
    /// Byte ranges of unresolved identifier occurrences spelled `name`.
    /// Remote (other-namespace) path segments are excluded: they cannot
    /// refer to a lexical binding.
    pub fn unresolved_named<'a>(&'a self, contents: &'a str, name: &str) -> Vec<Range<usize>> {
        self.refs
            .iter()
            .filter(|r| {
                r.target.is_none()
                    && !r.remote
                    && r.module_attr.is_none()
                    && &contents[r.name.clone()] == name
            })
            .map(|r| r.name.clone())
            .collect()
    }

    /// References resolving to `binding`, in source order.
    pub fn refs_of(&self, binding: usize) -> Vec<&RefSite> {
        let mut out: Vec<&RefSite> =
            self.refs.iter().filter(|r| r.target == Some(binding)).collect();
        out.sort_by_key(|r| r.name.start);
        out
    }
}
