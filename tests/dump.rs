use tree_sitter::Parser;
#[test]
fn dump() {
    let src = std::env::var("DUMP_SRC").unwrap();
    let lang: tree_sitter::Language = tree_sitter_objc::LANGUAGE.into();
    let mut p = Parser::new();
    p.set_language(&lang).unwrap();
    let t = p.parse(&src, None).unwrap();
    eprintln!("err={}", t.root_node().has_error());
    eprintln!("{}", t.root_node().to_sexp());
}
