use pwe_reference::lang::LangRuntime;
fn main() {
    let src = std::fs::read_to_string("/tmp/q.pwe").unwrap();
    match LangRuntime::compile(&src) {
        Ok(_) => println!("OK"),
        Err(e) => println!("ERR {:?}", e),
    }
}
