use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::SupportLang;
use std::str::FromStr;

#[test]
fn dump_sexp() {
    let lang = std::env::var("DUMP_LANG").unwrap_or_else(|_| "Lua".into());
    let src = std::env::var("DUMP_SRC").unwrap();
    let l = SupportLang::from_str(&lang).unwrap();
    eprintln!("{}", l.ast_grep(&src).root().get_inner_node().to_sexp());
}
