//! Cross-file (workspace) rename support for REFAC-8. Builds an
//! invocation-local module graph over the analyzed files: import edges
//! and `mod` declarations resolve to concrete files, symbol links are
//! followed transitively, and a rename expands from the defining
//! binding to every mention of the same *spelling*. Nothing persists;
//! every resolution is recomputed from in-memory sources per invocation.
//!
//! Mention rule: an import edge or module-attribute reference produces
//! an edit only when its imported/attribute spelling equals the renamed
//! name AND it resolves to the selected symbol. Aliased forms
//! (`import { x as y }`, `export { x as y }`, `import x from`,
//! `use a::x as y`) preserve the other spelling, so only the
//! module-side token is rewritten.

use std::path::{Path, PathBuf};

use crate::bind::{Analysis, ImportEdge};
use crate::lang::LangSpec;
use crate::source::SourceFile;

/// One analyzed file plus its language spec and contents.
pub(crate) struct FileCtx<'a> {
    pub file: &'a SourceFile,
    pub spec: &'a LangSpec,
    pub analysis: &'a Analysis,
}

/// What an import edge's `(specifier, name)` resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberTarget {
    /// A named symbol: (file index, binding index).
    Symbol(usize, usize),
    /// A module object (submodule, `import m`, `use a::b` where `b` is
    /// a module). Module objects are not rename symbols.
    Module(usize),
    /// The specifier looks internal (relative/crate/self/super/mod
    /// path) but resolution failed or was ambiguous — may hide the
    /// renamed symbol, so spelled-`old` occurrences block.
    Unresolved,
    /// External module (bare specifier, extern crate, std) — can never
    /// name an analyzed file's symbol.
    External,
}

/// A resolved mention of the renamed symbol in one file.
#[derive(Clone, Debug)]
pub(crate) struct Mention {
    /// File index into the analyzed set.
    pub file: usize,
    /// Byte range of the token to rewrite.
    pub range: std::ops::Range<usize>,
    /// Local binding created by an *unaliased* import edge whose name
    /// token coincides with `range`; its resolved references rename too.
    pub local_binding: Option<usize>,
}

fn is_js(spec: &LangSpec) -> bool {
    matches!(spec.id, "javascript" | "typescript" | "tsx")
}

fn is_py(spec: &LangSpec) -> bool {
    spec.id == "python"
}

/// Whether a specifier can only ever name external code for this
/// language family (bare JS imports, non-`::`-rooted Rust paths in
/// 2018+, absolute Python dots handled separately).
fn specifier_external(family: &str, specifier: &str) -> bool {
    match family {
        "javascript" | "typescript" | "tsx" => {
            !(specifier.starts_with("./") || specifier.starts_with("../"))
        }
        "rust" => {
            // `crate::`, `self::`, `super::` and paths whose first
            // segment matches a local `mod` are internal; everything
            // else *might* be an extern crate — resolution decides.
            !(specifier.starts_with("crate::")
                || specifier.starts_with("self::")
                || specifier.starts_with("super::"))
        }
        "python" => false, // dotted names may match analyzed files
        _ => true,
    }
}

/// Resolve a module specifier to an analyzed-file index.
/// `None` = external or unresolvable; callers distinguish via
/// `specifier_external`.
pub(crate) fn resolve_module(ctxs: &[FileCtx<'_>], from: usize, specifier: &str) -> Option<usize> {
    let family = ctxs[from].spec.id;
    let _ = family;
    if is_js(ctxs[from].spec) {
        resolve_js(ctxs, from, specifier)
    } else if is_py(ctxs[from].spec) {
        resolve_py(ctxs, from, specifier)
    } else if ctxs[from].spec.id == "rust" {
        resolve_rs_module(ctxs, from, specifier)
    } else {
        None
    }
}

/// Path comparison tolerant of `./` prefixes and `.`/`..` segments.
fn same_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                std::path::Component::CurDir => None,
                _ => Some(format!("{:?}", c)),
            })
            .collect()
    }
    norm(a) == norm(b)
}

/// JS/TS: only relative specifiers (`./x`, `../x`) name analyzed files.
/// Probe the specifier verbatim, `<base>.{ts,tsx,js,jsx,mjs,cjs}` then
/// `<base>/index.*`.
fn resolve_js(ctxs: &[FileCtx<'_>], from: usize, specifier: &str) -> Option<usize> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let base_dir = ctxs[from].file.path.parent()?;
    let joined = base_dir.join(specifier);
    let mut parts: Vec<String> = Vec::new();
    let mut absolute = false;
    for comp in joined.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            std::path::Component::RootDir => absolute = true,
            std::path::Component::Prefix(_) => return None,
        }
    }
    let stem = if absolute {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    };
    let mut candidates: Vec<String> = vec![stem.clone()];
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        candidates.push(format!("{stem}.{ext}"));
    }
    let mut index_candidates: Vec<String> = Vec::new();
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        index_candidates.push(format!("{stem}/index.{ext}"));
    }
    let mut hits: Vec<usize> = Vec::new();
    for cand in &candidates {
        for (i, c) in ctxs.iter().enumerate() {
            if is_js(c.spec) && same_path(&c.file.path, Path::new(cand)) {
                hits.push(i);
            }
        }
        if !hits.is_empty() {
            break;
        }
    }
    if hits.is_empty() {
        for cand in &index_candidates {
            for (i, c) in ctxs.iter().enumerate() {
                if is_js(c.spec) && same_path(&c.file.path, Path::new(cand)) {
                    hits.push(i);
                }
            }
            if !hits.is_empty() {
                break;
            }
        }
    }
    match hits.len() {
        1 => Some(hits[0]),
        _ => None,
    }
}

/// Python dotted module id for a file: `pkg/mod.py` → `pkg.mod`,
/// `pkg/__init__.py` → `pkg`, `mod.py` → `mod`.
fn py_module_id(path: &Path) -> Option<String> {
    let mut comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let file = comps.pop()?;
    let stem = file.rsplit_once('.').map_or(file.as_str(), |(s, _)| s);
    if stem != "__init__" {
        comps.push(stem.to_string());
    }
    if comps.is_empty() {
        None
    } else {
        Some(comps.join("."))
    }
}

fn py_is_package(path: &Path) -> bool {
    path.file_name()
        .map(|n| {
            let n = n.to_string_lossy();
            n == "__init__.py" || n == "__init__.pyi"
        })
        .unwrap_or(false)
}

fn resolve_py(ctxs: &[FileCtx<'_>], from: usize, specifier: &str) -> Option<usize> {
    let dots = specifier.bytes().take_while(|&b| b == b'.').count();
    let tail = &specifier[dots..];
    let want_owned;
    let want = if dots > 0 {
        let from_id = py_module_id(&ctxs[from].file.path)?;
        let mut segs: Vec<&str> = from_id.split('.').collect();
        if !py_is_package(&ctxs[from].file.path) {
            segs.pop();
        }
        for _ in 1..dots {
            segs.pop()?;
        }
        if !tail.is_empty() {
            segs.push(tail);
        }
        want_owned = segs.join(".");
        want_owned.as_str()
    } else {
        tail
    };
    if want.is_empty() {
        return None;
    }
    let mut hits: Vec<usize> = Vec::new();
    for (i, c) in ctxs.iter().enumerate() {
        if !is_py(c.spec) {
            continue;
        }
        let Some(id) = py_module_id(&c.file.path) else {
            continue;
        };
        if id == want || id.ends_with(&format!(".{want}")) {
            hits.push(i);
        }
    }
    match hits.len() {
        1 => Some(hits[0]),
        _ => None,
    }
}

// ---- Rust module tree -------------------------------------------------

fn rs_root_dir(ctxs: &[FileCtx<'_>], file_idx: usize) -> Option<PathBuf> {
    let file = &ctxs[file_idx].file.path;
    let name = file.file_name()?.to_string_lossy();
    if name == "lib.rs" || name == "main.rs" {
        return Some(file.parent()?.to_path_buf());
    }
    let mut dir = file.parent()?;
    loop {
        for c in ctxs {
            if c.spec.id != "rust" {
                continue;
            }
            let n = c.file.path.file_name().map(|n| n.to_string_lossy().into_owned());
            if matches!(n.as_deref(), Some("lib.rs") | Some("main.rs"))
                && c.file.path.parent() == Some(dir)
            {
                return Some(dir.to_path_buf());
            }
        }
        dir = dir.parent()?;
    }
}

/// Module path (`a::b`) of an analyzed Rust file relative to `root_dir`.
fn rs_module_path(root_dir: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root_dir).ok()?;
    let mut comps: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let last = comps.pop()?;
    let stem = last.strip_suffix(".rs").unwrap_or(&last);
    if !matches!(stem, "mod" | "lib" | "main") {
        comps.push(stem.to_string());
    }
    Some(comps.join("::"))
}

fn rs_file_for_module(ctxs: &[FileCtx<'_>], root_dir: &Path, module: &str) -> Option<usize> {
    let mut hits: Vec<usize> = Vec::new();
    for (i, c) in ctxs.iter().enumerate() {
        if c.spec.id != "rust" {
            continue;
        }
        if let Some(mp) = rs_module_path(root_dir, &c.file.path)
            && mp == module
        {
            hits.push(i);
        }
    }
    match hits.len() {
        1 => Some(hits[0]),
        _ => None,
    }
}

/// Resolve a Rust `::` path that names a *module* (no trailing symbol).
/// `self`/`super`/`crate` are relative to the importing file's module.
fn resolve_rs_module(ctxs: &[FileCtx<'_>], from: usize, specifier: &str) -> Option<usize> {
    let root = rs_root_dir(ctxs, from)?;
    let segs: Vec<&str> = specifier.split("::").filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return None;
    }
    let module = match segs[0] {
        "crate" => segs[1..].join("::"),
        "self" => {
            let cur = rs_module_path(&root, &ctxs[from].file.path)?;
            let tail = segs[1..].join("::");
            if tail.is_empty() {
                cur
            } else if cur.is_empty() {
                tail
            } else {
                format!("{cur}::{tail}")
            }
        }
        "super" => {
            let cur = rs_module_path(&root, &ctxs[from].file.path)?;
            let mut v: Vec<&str> = cur.split("::").filter(|s| !s.is_empty()).collect();
            v.pop()?;
            let tail = segs[1..].join("::");
            if tail.is_empty() {
                v.join("::")
            } else if v.is_empty() {
                tail
            } else {
                format!("{}::{tail}", v.join("::"))
            }
        }
        _ => {
            // Bare path: the whole thing may be a module path relative
            // to crate root, OR relative to the current module (path
            // expressions like `m::f()` inside `mod inner`). Try root
            // first, then current-module-relative.
            let joined = segs.join("::");
            if rs_file_for_module(ctxs, &root, &joined).is_some() {
                return rs_file_for_module(ctxs, &root, &joined);
            }
            let cur = rs_module_path(&root, &ctxs[from].file.path)?;
            if cur.is_empty() {
                return None;
            }
            format!("{cur}::{joined}")
        }
    };
    rs_file_for_module(ctxs, &root, &module)
}

// ---- name resolution --------------------------------------------------

/// Look up `name` inside module file `file`: explicit export aliases,
/// module-level bindings, named re-export edges (matched on the
/// *exported* spelling), then wildcard re-exports (unique candidate
/// wins, ambiguity → Unresolved).
pub(crate) fn resolve_member(
    ctxs: &[FileCtx<'_>],
    file: usize,
    name: &str,
    depth: usize,
) -> MemberTarget {
    if depth > 8 {
        return MemberTarget::Unresolved;
    }
    let a = ctxs[file].analysis;
    for e in &a.exports {
        if e.exported == name {
            return match e.local {
                Some(l) => MemberTarget::Symbol(file, l),
                None => MemberTarget::Unresolved,
            };
        }
    }
    for (i, b) in a.bindings.iter().enumerate() {
        if b.module_level && b.text == name {
            return MemberTarget::Symbol(file, i);
        }
    }
    // Named re-export edges: match on the exported spelling.
    for edge in &a.imports {
        if !edge.reexport || edge.star {
            continue;
        }
        let exported = edge.exported.as_deref().unwrap_or(&edge.name);
        if exported != name {
            continue;
        }
        return resolve_edge(ctxs, file, edge, depth + 1);
    }
    // Wildcard re-exports.
    let mut found: Option<MemberTarget> = None;
    let mut ambiguous = false;
    for edge in &a.imports {
        if !(edge.reexport && edge.star) {
            continue;
        }
        let Some(mf) = resolve_module(ctxs, file, &edge.specifier) else {
            continue;
        };
        match resolve_member(ctxs, mf, name, depth + 1) {
            MemberTarget::Symbol(..) | MemberTarget::Module(_) => {
                let c = resolve_member(ctxs, mf, name, depth + 1);
                if found.is_some() && found != Some(c) {
                    ambiguous = true;
                }
                found = Some(c);
            }
            _ => {}
        }
    }
    if ambiguous {
        MemberTarget::Unresolved
    } else {
        found.unwrap_or(MemberTarget::Unresolved)
    }
}

/// Symbol-only view of `resolve_member`.
pub(crate) fn resolve_name(
    ctxs: &[FileCtx<'_>],
    file: usize,
    name: &str,
    depth: usize,
) -> Option<(usize, usize)> {
    match resolve_member(ctxs, file, name, depth) {
        MemberTarget::Symbol(f, b) => Some((f, b)),
        _ => None,
    }
}

/// Resolve one import edge to its target member.
pub(crate) fn resolve_edge(
    ctxs: &[FileCtx<'_>],
    from: usize,
    edge: &ImportEdge,
    depth: usize,
) -> MemberTarget {
    if depth > 8 {
        return MemberTarget::Unresolved;
    }
    // Sentinel specifier: dynamic `import(expr)`/`require(expr)` — the
    // module is unknowable, so any spelled attribute may refer to the
    // renamed symbol.
    if edge.specifier.starts_with('\u{0}') {
        return MemberTarget::Unresolved;
    }
    let family = ctxs[from].spec.id;
    let Some(target_file) = resolve_module(ctxs, from, &edge.specifier) else {
        return if specifier_external(family, &edge.specifier) {
            MemberTarget::External
        } else {
            MemberTarget::Unresolved
        };
    };
    if edge.name.is_empty() {
        return MemberTarget::Module(target_file);
    }
    // Python: `from pkg import sub` may name a submodule rather than a
    // binding in the package __init__.
    if is_py(ctxs[target_file].spec)
        && let Some(id) = py_module_id(&ctxs[target_file].file.path) {
            let sub = format!("{id}.{}", edge.name);
            let mut hits = 0;
            for c in ctxs.iter() {
                if is_py(c.spec)
                    && let Some(cid) = py_module_id(&c.file.path)
                    && (cid == sub || cid.ends_with(&format!(".{sub}")))
                {
                    hits += 1;
                }
            }
            if hits == 1 {
                // Only treat as module when the package has no explicit
                // binding of the same name (explicit wins).
                let has_binding = ctxs[target_file]
                    .analysis
                    .bindings
                    .iter()
                    .any(|b| b.module_level && b.text == edge.name);
                if !has_binding {
                    for (i, c) in ctxs.iter().enumerate() {
                        if is_py(c.spec)
                            && let Some(cid) = py_module_id(&c.file.path)
                            && (cid == sub || cid.ends_with(&format!(".{sub}")))
                        {
                            return MemberTarget::Module(i);
                        }
                    }
                }
            }
        }
    // Rust: `use a::b` / path tail `b` may name a child module file.
    if ctxs[target_file].spec.id == "rust" {
        let root = match rs_root_dir(ctxs, target_file) {
            Some(r) => r,
            None => return MemberTarget::Unresolved,
        };
        if let Some(mp) = rs_module_path(&root, &ctxs[target_file].file.path) {
            let child = if mp.is_empty() {
                edge.name.clone()
            } else {
                format!("{mp}::{}", edge.name)
            };
            if let Some(f) = rs_file_for_module(ctxs, &root, &child) {
                let has_binding = ctxs[target_file]
                    .analysis
                    .bindings
                    .iter()
                    .any(|b| b.module_level && b.text == edge.name);
                if !has_binding {
                    return MemberTarget::Module(f);
                }
            }
        }
    }
    match resolve_name(ctxs, target_file, &edge.name, depth + 1) {
        Some((f, b)) => MemberTarget::Symbol(f, b),
        None => {
            if specifier_external(family, &edge.specifier) {
                MemberTarget::External
            } else {
                MemberTarget::Unresolved
            }
        }
    }
}

/// Canonical defining binding for `binding`: follow the import edge
/// that created it (unaliased or aliased — both denote the remote
/// symbol) to the definition, transitively. `None` = not import-derived
/// or edge external/unresolved (the binding is its own symbol).
pub(crate) fn canonical(
    ctxs: &[FileCtx<'_>],
    file: usize,
    binding: usize,
    depth: usize,
) -> Option<(usize, usize)> {
    if depth > 8 {
        return None;
    }
    for edge in &ctxs[file].analysis.imports {
        if edge.local == Some(binding) && !edge.name.is_empty() && !edge.star {
            match resolve_edge(ctxs, file, edge, depth + 1) {
                MemberTarget::Symbol(f, b) => {
                    if let Some(deeper) = canonical(ctxs, f, b, depth + 1) {
                        return Some(deeper);
                    }
                    return Some((f, b));
                }
                _ => return None,
            }
        }
    }
    None
}

/// Group same-spelling bindings into symbol clusters via `canonical`.
pub(crate) fn symbol_clusters(
    ctxs: &[FileCtx<'_>],
    name: &str,
) -> Vec<Vec<(usize, usize)>> {
    let mut clusters: Vec<Vec<(usize, usize)>> = Vec::new();
    'outer: for (fi, c) in ctxs.iter().enumerate() {
        for (bi, b) in c.analysis.bindings.iter().enumerate() {
            if b.text != name {
                continue;
            }
            let cand = (fi, bi);
            let canon = canonical(ctxs, fi, bi, 0).unwrap_or(cand);
            for cluster in &mut clusters {
                let rep_canon = canonical(ctxs, cluster[0].0, cluster[0].1, 0)
                    .unwrap_or(cluster[0]);
                if rep_canon == canon || cluster.contains(&canon) || cluster.contains(&cand) {
                    cluster.push(cand);
                    continue 'outer;
                }
            }
            clusters.push(vec![cand]);
        }
    }
    clusters
}

/// Collect every mention of the symbol defined at
/// `(def_file, def_binding)`: the definition's decl+refs, import edges
/// whose imported spelling equals the symbol name and resolve to it,
/// re-export edges, and module-attribute references.
pub(crate) fn collect_mentions(
    ctxs: &[FileCtx<'_>],
    def_file: usize,
    def_binding: usize,
) -> (Vec<Mention>, Vec<(usize, std::ops::Range<usize>)>) {
    let mut mentions = Vec::new();
    let mut blockers = Vec::new(); // (file, range) unresolved-relevant
    let def_name = ctxs[def_file].analysis.bindings[def_binding]
        .text
        .clone();

    mentions.push(Mention {
        file: def_file,
        range: ctxs[def_file].analysis.bindings[def_binding]
            .name
            .clone(),
        local_binding: None,
    });
    for r in ctxs[def_file].analysis.refs_of(def_binding) {
        mentions.push(Mention {
            file: def_file,
            range: r.name.clone(),
            local_binding: None,
        });
    }

    for (fi, c) in ctxs.iter().enumerate() {
        let a = c.analysis;
        for edge in &a.imports {
            if edge.star || edge.name.is_empty() {
                continue;
            }
            if edge.name != def_name {
                continue;
            }
            match resolve_edge(ctxs, fi, edge, 0) {
                MemberTarget::Symbol(f, b)
                    if (f, b) == (def_file, def_binding)
                        || canonical(ctxs, f, b, 0) == Some((def_file, def_binding)) =>
                {
                    mentions.push(Mention {
                        file: fi,
                        range: edge.name_range.clone(),
                        local_binding: edge.local,
                    });
                }
                MemberTarget::Unresolved => {
                    blockers.push((fi, edge.name_range.clone()));
                }
                _ => {}
            }
        }
        // Module-attribute references.
        for r in &a.refs {
            let Some(ei) = r.module_attr else {
                continue;
            };
            let edge = &a.imports[ei];
            let attr_name = c.file.contents[r.name.clone()].to_string();
            if attr_name != def_name {
                continue;
            }
            match resolve_edge(ctxs, fi, edge, 0) {
                MemberTarget::Module(mf) => {
                    if let Some((f, b)) = resolve_name(ctxs, mf, &attr_name, 0)
                        && ((f, b) == (def_file, def_binding)
                            || canonical(ctxs, f, b, 0) == Some((def_file, def_binding)))
                    {
                        mentions.push(Mention {
                            file: fi,
                            range: r.name.clone(),
                            local_binding: None,
                        });
                    }
                }
                MemberTarget::Unresolved => {
                    blockers.push((fi, r.name.clone()));
                }
                _ => {}
            }
        }
    }
    (mentions, blockers)
}
