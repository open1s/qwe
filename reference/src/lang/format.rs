//! Canonical source formatting (`pwe fmt`).
//!
//! The formatter is **token-preserving**: it only changes leading indentation
//! (4 spaces per `{}` block level), strips trailing whitespace, collapses runs
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

        // Mask string contents (replace them with spaces) so braces and `#`
        // inside strings do not affect indentation or comment stripping.
        let mut masked = String::with_capacity(raw.len());
        for ch in raw.chars() {
            if in_string {
                masked.push(' ');
                if ch == '"' {
                    in_string = false;
                }
            } else if ch == '"' {
                masked.push(' ');
                in_string = true;
            } else {
                masked.push(ch);
            }
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
        let code = code.split('#').next().unwrap_or("").trim_end();
        let lead_close = code.starts_with('}') || code.starts_with(')') || code.starts_with(']');
        let indent = if lead_close {
            depth.saturating_sub(1)
        } else {
            depth
        };
        for _ in 0..indent {
            out.push_str("    ");
        }
        out.push_str(content);
        out.push('\n');
        wrote_any = true;

        let opens = code.matches('{').count();
        let closes = code.matches('}').count();
        depth = depth.saturating_sub(closes);
        depth += opens;
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
        assert!(out.contains("\n    entity e {"), "depth corrupted:\n{out}");
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
        // Indentation is 4 spaces per level.
        assert!(out.contains("\n    entity e {"));
        assert!(out.contains("\n        state = (x=1.0)"));
    }
}
