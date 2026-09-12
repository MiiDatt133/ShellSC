use shell_ir::lowering::Lowerer;
use shell_lex::Lexer;
use shell_parse::Parser;

#[test]
fn procsub_parses_to_procsub_word_part() {
    let src = "diff <(echo a) <(echo b)\n";
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    let script = Parser::new(tokens).parse().expect("parse failed");
    let found = format!("{:?}", script).contains("ProcSub");
    assert!(found, "AST missing ProcSub for {src}");
}

#[test]
fn procsub_lowers_to_procsub_markers() {
    let src = "read b < <(echo second)\n";
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    let script = Parser::new(tokens).parse().expect("parse failed");
    let ir = Lowerer::new().lower(&script).expect("lower failed");
    let dbg = format!("{:?}", ir);
    assert!(dbg.contains("ProcSubBegin"), "missing ProcSubBegin in IR");
    assert!(dbg.contains("ProcSubEnd"), "missing ProcSubEnd in IR");
}
