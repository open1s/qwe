//! Documentation testing (`pwe doctest`).
//!
//! Extracts fenced code blocks from Markdown and compiles the ones that are
//! **complete programs** (a ```` ```pwe ```` block whose body starts with
//! `world`). Illustrative fragments are skipped. This keeps the tutorial honest:
//! a block that stops compiling is caught by `pwe doctest` (and the test suite).

/// A fenced code block: its 1-based start line, info string, and body.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub line: usize,
    pub info: String,
    pub code: String,
}

/// Extracts all fenced code blocks (``` … ```) from Markdown.
pub fn fenced_blocks(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut info = String::new();
    let mut body = String::new();
    let mut start = 0usize;
    for (i, line) in markdown.lines().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim_start();
        if !in_fence {
            if let Some(rest) = trimmed.strip_prefix("```") {
                in_fence = true;
                info = rest.trim().to_string();
                body.clear();
                start = lineno;
            }
        } else if trimmed.starts_with("```") {
            in_fence = false;
            blocks.push(Block {
                line: start,
                info: info.clone(),
                code: body.clone(),
            });
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    blocks
}

/// Whether a block is a runnable program: a `pwe` block that starts with `world`
/// and is not marked illustrative (`ignore` / `no-run` in the info string).
/// Blocks that use namespaced `std` calls without an import belong here as
/// `ignore` — they are fragments for illustration, not standalone programs.
pub fn is_runnable(block: &Block) -> bool {
    block.info.starts_with("pwe")
        && !block.info.contains("ignore")
        && !block.info.contains("no-run")
        && block.code.trim_start().starts_with("world")
}

/// A block that failed to compile.
#[derive(Debug, Clone)]
pub struct Failure {
    pub line: usize,
    pub error: String,
}

/// Compiles every runnable block, returning the failures.
pub fn check_document(markdown: &str) -> Vec<Failure> {
    let mut failures = Vec::new();
    for block in fenced_blocks(markdown) {
        if !is_runnable(&block) {
            continue;
        }
        if let Err(e) = crate::lang::LangRuntime::compile(&block.code) {
            failures.push(Failure {
                line: block.line,
                error: crate::lang::diagnose(&block.code, &e),
            });
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Title\n\n```pwe\nworld { gravity=(0,0,0) entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = x + inte(1.0) } }\n```\n\n```pwe\nsystems { update { on=e } }\n```\n\n```text\nnot code\n```\n";

    #[test]
    fn shipped_docs_compile() {
        for rel in [
            "../docs/lang-usage.md",
            "../README.md",
            "../docs/lang-usage.zh.md",
            "../README-ZH.md",
        ] {
            let path = concat!(env!("CARGO_MANIFEST_DIR"), "/").to_string() + rel;
            let md = std::fs::read_to_string(&path).unwrap_or_default();
            if md.is_empty() {
                continue;
            }
            let failures = check_document(&md);
            assert!(failures.is_empty(), "{rel}: {failures:?}");
        }
    }

    #[test]
    fn extracts_and_selects_runnable_blocks() {
        let blocks = fenced_blocks(MD);
        assert_eq!(blocks.len(), 3);
        let runnable: Vec<_> = blocks.iter().filter(|b| is_runnable(b)).collect();
        assert_eq!(runnable.len(), 1, "only the complete `world` program runs");
    }

    #[test]
    fn ignored_blocks_are_skipped() {
        let md = "```pwe ignore\nworld { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n```\n";
        assert!(
            check_document(md).is_empty(),
            "ignored blocks must not be checked"
        );
    }

    #[test]
    fn good_block_passes_bad_block_fails() {
        assert!(check_document(MD).is_empty(), "the good block compiles");
        let bad = "```pwe\nworld { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n```\n";
        let failures = check_document(bad);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.contains("nope"));
    }
}
