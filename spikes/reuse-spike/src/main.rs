//! REFAC-5 reuse spike: executable evidence for the parser/resolver choice.
//!
//! Each section is a small, real case. Results are printed; nothing here is
//! wired into fastermod.

fn main() {
    tree_sitter_spike();
    ast_grep_spike();
    oxc_spike();
}

// ---------- tree-sitter: CST + byte spans for identifier roles ----------

fn tree_sitter_spike() {
    println!("== tree-sitter ==");
    let source = "fn main() {\n    let counter = 1;\n    println!(\"{counter}\");\n}\n";
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let root = tree.root_node();
    println!(
        "rust parse: has_error={} bytes={}-{}",
        root.has_error(),
        root.start_byte(),
        root.end_byte()
    );
    // Find every `identifier` node: these are the token-level candidates
    // for --ident / binding rename.
    let mut cursor = root.walk();
    let mut idents = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" {
            idents.push((node.start_byte(), node.end_byte(), node.utf8_text(source.as_bytes()).unwrap().to_string()));
        }
        let mut c = node.walk();
        for child in node.children(&mut c) {
            stack.push(child);
        }
        let _ = &mut cursor;
    }
    idents.sort();
    for (s, e, text) in &idents {
        println!("  ident {text:?} bytes {s}..{e}");
    }
    // Python + JS grammars load and parse too.
    for (name, lang) in [
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("javascript", tree_sitter_javascript::LANGUAGE.into()),
    ] {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&lang).unwrap();
        let sample = if name == "python" { "x = 1\n" } else { "let x = 1;\n" };
        let t = p.parse(sample, None).unwrap();
        println!("  {name}: has_error={}", t.root_node().has_error());
    }
    // TypeScript grammar crate bundles tsx via language_typescript.
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .unwrap();
    let t = p.parse("const x: number = 1;\n", None).unwrap();
    println!("  typescript: has_error={}", t.root_node().has_error());
}

// ---------- ast-grep-core: structural pattern match + rewrite ----------

fn ast_grep_spike() {
    println!("== ast-grep-core ==");
    use ast_grep_language::{Language, LanguageExt, SupportLang};

    // SupportLang::from_path also gives extension->language detection.
    let lang: SupportLang = "rs".parse().unwrap();
    let source = "fn caller() {\n    helper(a, b);\n    // helper(c) in a comment must not match\n}\n";
    let root = lang.ast_grep(source);
    // A pattern string is itself a Matcher; $$$ARGS is the multi-metavar.
    let matches: Vec<_> = root.root().find_all("helper($$$ARGS)").collect();
    println!("  rust pattern matches: {}", matches.len());
    for m in &matches {
        println!(
            "    match bytes {:?} text {:?}",
            m.range(),
            m.text()
        );
    }
    // Structural rewrite: same pattern, template replacement.
    let edits = root.root().replace_all("helper($$$ARGS)", "renamed($$$ARGS)");
    println!("  rewrite edits produced: {}", edits.len());
    for e in &edits {
        println!("    edit: {:?}", e);
    }

    let py_lang: SupportLang = "py".parse().unwrap();
    let py_src = "def f():\n    helper(a, b)\n";
    let py_root = py_lang.ast_grep(py_src);
    let n = py_root.root().find_all("helper($$$ARGS)").count();
    println!("  python pattern matches: {n}");

    // Language detection from path is built in.
    println!(
        "  from_path main.rs -> {:?}, script.py -> {:?}",
        SupportLang::from_path("src/main.rs"),
        SupportLang::from_path("script.py")
    );
}

// ---------- oxc: JS/TS semantic scoping for binding rename ----------

fn oxc_spike() {
    println!("== oxc_semantic ==");
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;

    let source = "function f() {\n  let counter = 1;\n  return counter + 1;\n}\nf();\n";
    let allocator = Allocator::default();
    let source_type = SourceType::default().with_module(true);
    let ret = Parser::new(&allocator, source, source_type).parse();
    println!("  js parse diagnostics: {}", ret.diagnostics.len());
    let semantic = SemanticBuilder::new().build(&ret.program).semantic;
    let scoping = semantic.scoping();
    println!(
        "  semantic: {} scopes, {} symbols, {} references",
        scoping.scopes_len(),
        scoping.symbols_len(),
        scoping.references_len()
    );
    // Find the `counter` binding: declaration span plus resolved references
    // is exactly the data a local rename needs.
    for id in scoping.symbol_ids() {
        if scoping.symbol_name(id) == "counter" {
            let span = scoping.symbol_span(id);
            let refs = scoping.get_resolved_reference_ids(id);
            println!(
                "  symbol `counter` decl bytes {}..{} scope={:?} flags={:?} resolved_refs={}",
                span.start,
                span.end,
                scoping.symbol_scope_id(id),
                scoping.symbol_flags(id),
                refs.len()
            );
        }
    }
}
