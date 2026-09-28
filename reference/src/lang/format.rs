//! Canonical source formatting (`pwe fmt`).
//!
//! The formatter is **token-preserving**: it only changes leading indentation
//! (4 spaces per `{}` block level), strips trailing whitespace, collapses runs
//! of blank lines, and ensures a single trailing newline. Internal spacing and
//! comments (`# …`) are left untouched, so the token stream — and therefore the
//! parsed program and the compiled artifact — is identical. That property is
//! what the tests assert (idempotence + artifact-hash equality).

/// Reformats `src`; see the module docs for the guarantees.
pub fn format_source(src: &str) -> String {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut depth: usize = 0;
    let mut blank_pending = false;
    let mut wrote_any = false;

    for raw in normalized.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            if wrote_any {
                blank_pending = true;
            }
            continue;
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }
        let trimmed = line.trim_start();
        // Ignore `#` comments when measuring block depth (a comment may contain
        // braces) and when deciding whether the line starts with a closer.
        let code = trimmed.split('#').next().unwrap_or("").trim_end();
        let lead_close = code.starts_with('}') || code.starts_with(')') || code.starts_with(']');
        let indent = if lead_close {
            depth.saturating_sub(1)
        } else {
            depth
        };
        for _ in 0..indent {
            out.push_str("    ");
        }
        out.push_str(trimmed);
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
