//! Gate: every committed `.pwe` is already canonical, so `pwe fmt --check`
//! passes on the repo and the formatter's canonical style matches it.
use std::path::{Path, PathBuf};

fn pwe_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                pwe_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "pwe") {
                out.push(p);
            }
        }
    }
}

#[test]
fn committed_programs_are_formatted() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let mut files = Vec::new();
    pwe_files(&root.join("std"), &mut files);
    pwe_files(&root.join("cli/examples"), &mut files);
    assert!(!files.is_empty(), "no .pwe files found");
    let mut bad = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        if pwe_reference::lang::format_source(&src) != src {
            bad.push(f.display().to_string());
        }
    }
    assert!(bad.is_empty(), "not canonically formatted: {bad:#?}");
}
