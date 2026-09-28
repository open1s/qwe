//! Compile diagnostics: detail-code names, human messages, and source-caret
//! rendering. Kept separate from lowering so the diagnostic surface is stable.
use super::*;

pub(crate) fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

// ---------------------------------------------------------------------------
// Compile diagnostics
// ---------------------------------------------------------------------------

/// A single compile diagnostic: a human message, a detail code, and a byte
/// offset into the source (`0` when no precise position is available).
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub detail: u32,
    pub message: String,
    pub byte_offset: usize,
}

thread_local! {
    /// Diagnostics accumulated by the most recent compile on this thread.
    static DIAGNOSTICS: std::cell::RefCell<Vec<Diagnostic>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Clears the accumulated diagnostics for the current thread.
pub fn clear_diagnostics() {
    DIAGNOSTICS.with(|d| d.borrow_mut().clear());
}

/// Returns the diagnostics accumulated since the last `clear_diagnostics`,
/// clearing the log. The most recent entry is last.
pub fn take_diagnostics() -> Vec<Diagnostic> {
    DIAGNOSTICS.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

pub(crate) fn push_diag(detail: u32, byte_offset: usize, message: impl Into<String>) {
    DIAGNOSTICS.with(|d| {
        d.borrow_mut().push(Diagnostic {
            detail,
            byte_offset,
            message: message.into(),
        })
    });
}

/// An error with a human message and a byte offset, recorded as a diagnostic.
pub(crate) fn error_at(
    status: Status,
    detail: u32,
    byte_offset: usize,
    message: impl Into<String>,
) -> Error {
    push_diag(detail, byte_offset, message);
    Error {
        status,
        detail,
        byte_offset: byte_offset as u64,
    }
}

/// Maps a detail code to a short human phrase (used when no richer diagnostic
/// was recorded). Keep in sync with the detail-code table in `docs/lang-usage`.
pub fn detail_name(detail: u32) -> &'static str {
    match detail {
        48 => "missing required system parameter",
        49 => "unknown system kind",
        51 => "convex hull needs at least 4 points",
        52 => "state slot index out of range (0..=15)",
        53 => "missing linear system row",
        54 => "linear row length must equal slots + 1",
        55 => "invalid function body / empty update rule / bad slot lhs",
        56 => "expression parse failure",
        57 => "number parse failure",
        58 => "slot or reference parse failure",
        59 => "call arity mismatch",
        60 => "program parse failure",
        62 => "unknown entity or channel name",
        63 => "nbody system has no dynamic bodies",
        64 => "invalid color literal",
        65 => "invalid loop count (integer in 1..=1000 required)",
        66 => "loop unrolls beyond the 10000-statement limit",
        67 => "let name shadows a reserved token (t / pi / e / sN)",
        68 => "for range must be ascending integers",
        69 => "invariant violated at step boundary",
        70 => "spatial query outside a system rule (no entity context)",
        71 => "every must be an integer ≥ 1",
        72 => "substeps must be an integer in 1..=1000",
        73 => "dynamic slot LHS is update-only (rk4 stages need compile-time slots)",
        75 => "field needs width and height ≥ 1",
        76 => "import failed (missing file, bad directive, or cycle)",
        77 => "dimension mismatch (see declared units)",
        83 => "unsupported lang_version (see docs/lang-usage)",
        84 => "malformed unit annotation",
        85 => "unknown identifier (reads 0.0)",
        86 => "unstable solver setting (CFL / diffusion limit)",
        87 => "conserved quantity drifted beyond tolerance",
        88 => "non-finite state (simulation diverged)",
        89 => "type mismatch in a `let` annotation",
        90 => "unit annotation required (units = \"strict\")",
        91 => "nbody body needs at least 7 state slots (px,py,pz,vx,vy,vz,m)",
        92 => "nbody ignores the `mass` field (reads state[6])",
        93 => "plain assignment is `update`-only (rk4 integrates `inte slot = rate`)",
        94 => "state slot name collides with a system parameter name",
        95 => "state slots 7/8/9 used but `orient != true` (read as a Z-spin)",
        96 => "state slots 7/8/9 written without `orient = true` (read as a Z-spin)",
        97 => "GPU backend not built (rebuild with `--features gpu` on macOS)",
        98 => "no Metal GPU device available",
        99 => "Metal kernel failed to compile",
        100 => "assignment to an unknown state slot (ignored)",
        101 => "duplicate module name (two modules declare the same `module`)",
        102 => "reference to a non-exported module item",
        4 => "EIR operand/type validation failed (compiler bug)",
        6 => "EIR result type unspecified (compiler bug)",
        _ => "unspecified compile error",
    }
}

/// Renders a diagnostic as a readable multi-line message with the offending
/// source line and a caret. `source` is the original program text.
pub fn render_diagnostic(source: &str, diag: &Diagnostic) -> String {
    let mut out = format!("error {}: {}", diag.detail, diag.message);
    let off = diag.byte_offset;
    if off > 0 && off <= source.len() {
        let before = &source[..off];
        let line = before.matches('\n').count() + 1;
        let col = before.rfind('\n').map(|i| off - i).unwrap_or(off + 1);
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = source[off..]
            .find('\n')
            .map(|i| off + i)
            .unwrap_or(source.len());
        out.push_str(&format!("\n  --> line {line}, column {col}\n"));
        out.push_str(&format!(
            "    |\n{line:>4} | {}\n    | ",
            &source[line_start..line_end]
        ));
        for _ in 0..(col.saturating_sub(1)) {
            out.push(' ');
        }
        out.push('^');
    }
    out
}

/// Builds a human-readable diagnostic for a failed compile against `source`,
/// preferring the most recent recorded diagnostic for `err`'s detail code, then
/// falling back to the code's canonical phrase.
pub fn diagnose(source: &str, err: &Error) -> String {
    let diags = take_diagnostics();
    let found = diags.iter().rev().find(|d| d.detail == err.detail);
    match found {
        Some(d) => render_diagnostic(source, d),
        None => {
            let msg = format!("error {}: {}", err.detail, detail_name(err.detail));
            if err.byte_offset > 0 {
                let pseudo = Diagnostic {
                    detail: err.detail,
                    message: detail_name(err.detail).to_string(),
                    byte_offset: err.byte_offset as usize,
                };
                render_diagnostic(source, &pseudo)
            } else {
                msg
            }
        }
    }
}
