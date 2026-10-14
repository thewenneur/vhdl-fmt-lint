//! Prints the syntax tree and errors of one file, for debugging.
fn main() {
    let path = std::env::args().nth(1).expect("path");
    let text = std::fs::read_to_string(&path).expect("read");
    let std = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(vfl_verilog_syntax::default_standard_for_extension)
        .unwrap_or_default();
    let parse = vfl_verilog_syntax::parse(&text, std);
    print!("{}", parse.debug_tree());
    for e in parse.errors() {
        println!("error: {e}");
    }
}
