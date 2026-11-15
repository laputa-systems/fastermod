use tree_sitter::Parser;

fn dump(lang_name: &str, parser_lang: tree_sitter::Language, src: &str) {
    let mut p = Parser::new();
    p.set_language(&parser_lang).unwrap();
    let tree = p.parse(src, None).unwrap();
    let mut kinds = std::collections::BTreeSet::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        kinds.insert(n.kind().to_string());
        let mut c = n.walk();
        for ch in n.children(&mut c) { stack.push(ch); }
    }
    println!("== {lang_name}: {}", kinds.iter().filter(|k| k.contains("ident") || k.contains("type")).cloned().collect::<Vec<_>>().join(", "));
}

fn main() {
    dump("rust", tree_sitter_rust::LANGUAGE.into(), "fn f() { let my_var: MyType = other_fn(arg); self.field = x; }");
    dump("python", tree_sitter_python::LANGUAGE.into(), "def f():\n    my_var = MyClass(attr=1)\n    obj.attr = other\n");
    dump("javascript", tree_sitter_javascript::LANGUAGE.into(), "function f() { let myVar = new MyClass(); obj.prop = other; class C { #priv = 1; } }");
    dump("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "function f(x: MyType): Ret { let y = x as Cast; }");
    dump("tsx", tree_sitter_typescript::LANGUAGE_TSX.into(), "const el = <div className={cls}>hi</div>;");
}
