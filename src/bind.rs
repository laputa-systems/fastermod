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
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Analysis {
    pub bindings: Vec<BindingSite>,
    pub refs: Vec<RefSite>,
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
                r.target.is_none() && !r.remote && &contents[r.name.clone()] == name
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
