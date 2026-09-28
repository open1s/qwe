//! Documentation testing (`pwe doctest`).
//!
//! Extracts fenced code blocks from Markdown and compiles the ones that are
//! **complete programs** (a ```` ```pwe ```` block whose body starts with
//! `world`). Illustrative fragments are skipped (`pwe ignore`). It reports hard
//! compile errors, **warnings** (e.g. unknown identifiers), and **unclosed
//! fences** — so a doc typo or a truncated file cannot pass silently.

/// A fenced code block: its 1-based start line, info string, and body.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub line: usize,
    pub info: String,
    pub code: String,
}

/// Extraction result: the closed blocks, and the start lines of fences that
/// were never closed (a malformed document).
#[derive(Debug, Clone, Default)]
pub struct Extracted {
    pub blocks: Vec<Block>,
    pub unclosed: Vec<usize>,
}

/// Extracts all fenced code blocks (``` … ```). A closing fence must be at least
/// as long as its opener, so a 3-backtick block nested in a 4-backtick fence is
/// body, not a closer. An unterminated fence is reported in `unclosed`.
pub fn extract(markdown: &str) -> Extracted {
    let mut out = Extracted::default();
    let mut in_fence = false;
    let mut fence_len = 0usize;
    let mut info = String::new();
    let mut body = String::new();
    let mut start = 0usize;
    for (i, line) in markdown.lines().enumerate() {
        let lineno = i + 1;
        let t = line.trim_start();
        let ticks = t.chars().take_while(|c| *c == '`').count();
        if !in_fence {
            if ticks >= 3 {
                in_fence = true;
                fence_len = ticks;
                info = t[ticks..].trim().to_string();
                body.clear();
                start = lineno;
            }
        } else if ticks >= fence_len && t[ticks..].trim().is_empty() {
            in_fence = false;
            out.blocks.push(Block {
                line: start,
                info: info.clone(),
                code: body.clone(),
            });
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if in_fence {
        out.unclosed.push(start);
    }
    out
}

/// Extracts only the closed blocks (see [`extract`]).
pub fn fenced_blocks(markdown: &str) -> Vec<Block> {
    extract(markdown).blocks
}

/// Whether a block is a runnable program: a `pwe` block that starts with `world`
/// and is not marked illustrative (`ignore` / `no-run`). Blocks that use
/// namespaced `std` calls without an import belong here as `ignore`.
pub fn is_runnable(block: &Block) -> bool {
    block.info.starts_with("pwe")
        && !block.info.contains("ignore")
        && !block.info.contains("no-run")
        && first_program_line(&block.code)
            .map(|l| l.starts_with("world"))
            .unwrap_or(false)
}

/// The first source line that is not a directive (`module`/`export`/`import`)
/// or blank — so a block may open with directives before `world`.
fn first_program_line(code: &str) -> Option<&str> {
    code.lines().find_map(|l| {
        let t = l.trim();
        if t.is_empty()
            || t.starts_with("module ")
            || t.starts_with("export ")
            || t.starts_with("import ")
        {
            None
        } else {
            Some(t)
        }
    })
}

/// A hard defect (compile error or unclosed fence).
#[derive(Debug, Clone)]
pub struct Failure {
    pub line: usize,
    pub error: String,
}

/// A warning-level doc defect (e.g. detail 85/100 raised by a compiling block).
#[derive(Debug, Clone)]
pub struct Warning {
    pub line: usize,
    pub message: String,
}

/// Compiles every runnable block, returning failures and warnings. Unclosed
/// fences are failures.
pub fn check_document_full(markdown: &str) -> (Vec<Failure>, Vec<Warning>) {
    let ex = extract(markdown);
    let mut failures: Vec<Failure> = ex
        .unclosed
        .iter()
        .map(|line| Failure {
            line: *line,
            error: "unclosed code fence (missing ``` terminator)".to_string(),
        })
        .collect();
    let mut warnings = Vec::new();
    for block in &ex.blocks {
        if !is_runnable(block) {
            continue;
        }
        crate::lang::clear_diagnostics();
        match crate::lang::LangRuntime::compile(&block.code) {
            Ok(_) => {
                for d in crate::lang::take_diagnostics() {
                    warnings.push(Warning {
                        line: block.line,
                        message: format!("[{}] {}", d.detail, d.message),
                    });
                }
            }
            Err(e) => {
                failures.push(Failure {
                    line: block.line,
                    error: crate::lang::diagnose(&block.code, &e),
                });
            }
        }
        crate::lang::clear_diagnostics();
    }
    (failures, warnings)
}

/// The hard failures only (see [`check_document_full`]).
pub fn check_document(markdown: &str) -> Vec<Failure> {
    check_document_full(markdown).0
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# T\n\n```pwe\nworld { gravity=(0,0,0) entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = x + inte(1.0) } }\n```\n\n```pwe\nsystems { update { on=e } }\n```\n\n```text\nnope\n```\n";

    #[test]
    fn extracts_and_selects_runnable_blocks() {
        let ex = extract(MD);
        assert_eq!(ex.blocks.len(), 3);
        assert!(ex.unclosed.is_empty());
        assert_eq!(ex.blocks.iter().filter(|b| is_runnable(b)).count(), 1);
    }

    #[test]
    fn four_backtick_fence_wraps_inner_block() {
        // A ````md fence may contain a ```pwe block; the inner ``` is body.
        let md = "````md\n```pwe\nworld { entity e { state=(x=1.0) } }\n```\n````\n";
        let ex = extract(md);
        assert_eq!(ex.blocks.len(), 1);
        assert_eq!(ex.blocks[0].info, "md");
        assert!(ex.unclosed.is_empty());
    }

    #[test]
    fn unclosed_fence_is_a_failure() {
        let md = "```pwe\nworld { entity e { state=(x=1.0) } }\n";
        let (failures, _) = check_document_full(md);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.contains("unclosed"));
    }

    #[test]
    fn warning_only_block_is_reported() {
        let md = "```pwe\nworld { gravity=(0,0,0) entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = x + ghost + 1.0 } }\n```\n";
        let (failures, warnings) = check_document_full(md);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].message.contains("ghost"), "{warnings:?}");
    }

    #[test]
    fn ignored_blocks_are_skipped() {
        let md = "```pwe ignore\nworld { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n```\n";
        assert!(check_document(md).is_empty());
    }

    #[test]
    fn good_block_passes_bad_block_fails() {
        assert!(check_document(MD).is_empty());
        let bad = "```pwe\nworld { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = nope(1.0) } }\n```\n";
        let failures = check_document(bad);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.contains("nope"));
    }

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
            let (failures, warnings) = check_document_full(&md);
            assert!(failures.is_empty(), "{rel}: failures {failures:?}");
            assert!(warnings.is_empty(), "{rel}: warnings {warnings:?}");
        }
    }
}
