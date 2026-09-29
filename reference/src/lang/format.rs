//! Canonical source formatting (`pwe fmt`).
//!
//! The formatter is **token-preserving**: it only changes leading indentation
//! (2 spaces per `{}` block level), strips trailing whitespace, collapses runs
//! of blank lines, and ensures a single trailing newline. Internal spacing and
//! comments (`# …`) are left untouched, so the token stream — and therefore the
//! parsed program and the compiled artifact — is identical. That property is
//! what the tests assert (idempotence + artifact-hash equality).

/// Reformats `src`; see the module docs for the guarantees.
///
/// String-aware: characters inside `"…"` (including multi-line strings) are
/// never treated as code — `{`/`}`/`#` inside a string neither change block
/// depth nor start a comment — and continuation lines of a multi-line string
/// are emitted verbatim so their content is preserved exactly.
pub fn format_source(src: &str) -> String {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut depth: usize = 0;
    let mut blank_pending = false;
    let mut wrote_any = false;
    let mut in_string = false;

    for raw in normalized.lines() {
        if in_string {
            // Inside a multi-line string: preserve the line verbatim.
            out.push_str(raw);
            out.push('\n');
            wrote_any = true;
            blank_pending = false;
            if raw.contains('"') {
                in_string = false;
            }
            continue;
        }

        // Build a "code" view of the line: strings are blanked (so braces and
        // comment markers inside them do not count) and a comment (`#` or `//`)
        // ends the line (so quotes/braces inside a comment do not count either).
        let chars: Vec<char> = raw.chars().collect();
        let mut masked = String::with_capacity(raw.len());
        let mut ci = 0;
        while ci < chars.len() {
            let ch = chars[ci];
            if in_string {
                masked.push(' ');
                if ch == '"' {
                    in_string = false;
                }
                ci += 1;
                continue;
            }
            if ch == '#' || (ch == '/' && chars.get(ci + 1) == Some(&'/')) {
                break; // comment: ignore the rest of the line
            }
            if ch == '"' {
                masked.push(' ');
                in_string = true;
                ci += 1;
                continue;
            }
            masked.push(ch);
            ci += 1;
        }

        // If the line opened a string that has not closed, its trailing
        // whitespace is string content — keep it.
        let trimmed_end = if in_string { raw } else { raw.trim_end() };
        if trimmed_end.trim().is_empty() {
            if wrote_any {
                blank_pending = true;
            }
            continue;
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }

        let content = trimmed_end.trim_start();
        let code = masked.trim();
        let lead_close = code.starts_with('}') || code.starts_with(')') || code.starts_with(']');
        let indent = if lead_close {
            depth.saturating_sub(1)
        } else {
            depth
        };
        for _ in 0..indent {
            out.push_str("  ");
        }
        out.push_str(content);
        out.push('\n');
        wrote_any = true;

        // Net brace change with signed arithmetic so a balanced single-line
        // block (`world { entity e { … } }`) does not inflate the depth.
        let opens = code.matches('{').count() as isize;
        let closes = code.matches('}').count() as isize;
        depth = (depth as isize + opens - closes).max(0) as usize;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSY: &str = "world {\n       gravity = (0,-9.81,0)\n\n\n  entity e {\n        state = (x=1.0)\n     }\n}\nsystems {\nupdate {\n on=e\n dt=0.1\n x = x + 1.0   \n}\n}\n";

    #[test]
    fn format_is_idempotent() {
        let once = format_source(MESSY);
        assert_eq!(once, format_source(&once));
    }

    #[test]
    fn format_is_string_aware() {
        // A multi-line string with braces/# inside must be preserved verbatim,
        // and must not corrupt block depth for following lines.
        let src =
            "world {\n  title = \"hi\n  { # not code\n  there\"\n entity e { state=(x=0.0) }\n}\n";
        let out = format_source(src);
        assert!(
            out.contains("\"hi\n  { # not code\n  there\""),
            "string mutated:\n{out}"
        );
        // `entity e` is one block level in (inside `world {`), not corrupted.
        assert!(out.contains("\n  entity e {"), "depth corrupted:\n{out}");
    }

    #[test]
    fn format_is_comment_aware() {
        // A quote inside a `#` comment must not open a fake multi-line string,
        // and braces inside a `//` comment must not change block depth.
        let src = "world {\n  # a \" quote in a comment\n  // { { { comment\n entity e {\n  state=(x=0.0)\n }\n}\n";
        let out = format_source(src);
        assert!(out.contains("\n  entity e {"), "depth corrupted:\n{out}");
        assert!(
            out.contains("\n    state=(x=0.0)"),
            "depth corrupted:\n{out}"
        );
        // The comment lines are preserved (indented, content intact).
        assert!(out.contains("# a \" quote in a comment"));
        assert!(out.contains("// { { { comment"));
    }

    #[test]
    fn single_line_block_keeps_depth() {
        let src =
            "world { entity e { state=(x=1.0) } }\nsystems { update { on=e; dt=1.0 x = x } }\n";
        let out = format_source(src);
        // The second top-level line must stay at column 0.
        assert!(out.contains("\nsystems {"), "depth inflated:\n{out}");
    }

    #[test]
    fn format_preserves_tokens() {
        let out = format_source(MESSY);
        assert!(
            out.contains("gravity = (0,-9.81,0)"),
            "internal spacing kept"
        );
        assert!(out.ends_with('\n'));
        assert!(!out.contains("\n\n\n"), "blank runs collapsed");
        // Indentation is 2 spaces per level.
        assert!(out.contains("\n  entity e {"));
        assert!(out.contains("\n    state = (x=1.0)"));
    }
}
