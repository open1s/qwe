//! Source migration: upgrade pre-v0.3 PWE sources to the frozen v0.3 semantics.
//!
//! The v0.3 change was that `=` became **assignment**; the old implicit
//! integration `slot = rate` is now written `inte slot = rate`. The migration is
//! therefore:
//!
//! * `deriv(E)` (old: the `dt·E` increment)  → `inte(E)`
//! * `deriv slot = rate` (old statement)      → `inte slot = rate`
//! * `slot = rate` where `slot` is a state slot (old implicit integration)
//!   → `inte slot = rate`  (equivalent in both `update` and `rk4`)
//!
//! Rule slots are recognised from the *parsed* model (entity state names and the
//! system rules themselves), so system parameters (`dt`, `on`, `when`, …) are
//! never touched. A `lang_version = "0.3"` marker is added to the world block.
use super::*;
use std::collections::BTreeSet;

/// The result of migrating a source string.
pub struct Migration {
    /// The migrated source (or the input unchanged when already current).
    pub source: String,
    /// How many rule lines were rewritten to `inte … = …`.
    pub rules_migrated: usize,
    /// True when the input already declared `lang_version = "0.3"`.
    pub already_current: bool,
}

/// Replaces whole-word occurrences of `from` with `to` (identifier boundaries).
fn replace_word(input: &str, from: &str, to: &str) -> String {
    let bytes = input.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i..].starts_with(from)
            && (i == 0 || !is_id(bytes[i - 1]))
            && (i + from.len() >= input.len() || !is_id(bytes[i + from.len()]))
        {
            out.push_str(to);
            i += from.len();
        } else if let Some(ch) = input[i..].chars().next() {
            // Advance one UTF-8 char.
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// The `lang_version` value, if declared.
fn declared_version(src: &str) -> Option<String> {
    let idx = src.find("lang_version")?;
    let rest = &src[idx + "lang_version".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.trim_start_matches('"');
    Some(
        rest.chars()
            .take_while(|&c| c != '"')
            .collect::<String>()
            .trim()
            .to_string(),
    )
}

/// Inserts `lang_version = "0.3";` immediately after the first `world {`.
fn ensure_version(src: &str) -> (String, bool) {
    if declared_version(src).is_some() {
        return (src.to_string(), false);
    }
    match src.find("world {") {
        Some(i) => {
            let at = i + "world {".len();
            let mut out = String::with_capacity(src.len() + 24);
            out.push_str(&src[..at]);
            out.push_str(" lang_version = \"0.3\"");
            out.push_str(&src[at..]);
            (out, true)
        }
        None => (src.to_string(), false),
    }
}

/// Rewrites `slot = rate` (old implicit integration) to `inte slot = rate` for
/// lines inside a `systems { … }` block whose LHS is a rule slot.
fn rewrite_rules(src: &str, slots: &BTreeSet<String>) -> (String, usize) {
    let mut out = String::with_capacity(src.len());
    let mut in_systems = false;
    let mut depth: i32 = 0;
    let mut count = 0usize;
    for line in src.lines() {
        let trimmed = line.trim_start();
        let mut rewritten = None;
        if in_systems {
            // `LHS = RHS`, LHS being `name`, `name.field`, or `sN`/`s[…]`.
            if let Some(eq) = trimmed.find('=') {
                let (lhs, rhs) = trimmed.split_at(eq);
                let rhs = rhs[1..].trim_start(); // after '='
                let lhs = lhs.trim_end();
                let is_slot = !lhs.is_empty()
                    && !lhs.contains(' ')
                    && (slots.contains(lhs)
                        || lhs
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                            && (lhs.contains('.')
                                || lhs.starts_with("s[")
                                || lhs.matches(|c: char| c.is_ascii_digit()).count() > 0
                                || lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')));
                let already = trimmed.starts_with("inte ")
                    || trimmed.starts_with("integrate ")
                    || trimmed.starts_with("deriv ")
                    || trimmed.starts_with("let ")
                    || trimmed.starts_with("on ")
                    || trimmed.starts_with("when ")
                    || trimmed.starts_with("every ")
                    || trimmed.starts_with("substeps ")
                    || rhs.contains("inte(")
                    || rhs.contains("+=")
                    || lhs.is_empty();
                if is_slot && !already && !rhs.is_empty() {
                    let indent = &line[..line.len() - trimmed.len()];
                    rewritten = Some(format!("{indent}inte {lhs} = {rhs}"));
                }
            }
        }
        if let Some(r) = rewritten {
            out.push_str(&r);
            count += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
        // Track the `systems { … }` block.
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
            }
        }
        if !in_systems && line.contains("systems") && line.contains('{') && depth > 0 {
            in_systems = true;
        }
        if in_systems && depth <= 0 {
            in_systems = false;
        }
    }
    (out, count)
}

/// Migrates a pre-v0.3 source to the frozen v0.3 semantics.
pub fn migrate_v02_to_v03(input: &str) -> Result<Migration> {
    if declared_version(input).as_deref() == Some(LANG_VERSION) {
        return Ok(Migration {
            source: input.to_string(),
            rules_migrated: 0,
            already_current: true,
        });
    }
    // 1. The old `deriv(E)` operator and `deriv slot = rate` statement.
    let step1 = replace_word(&input.replace("deriv(", "inte("), "deriv", "inte");
    // 2. Learn the rule slots from the parsed model: entity state names plus the
    //    system rules the parser recognised.
    let parsed = parse(&step1)?;
    let mut slots: BTreeSet<String> = BTreeSet::new();
    for e in &parsed.model.entities {
        if let Some(names) = &e.state_names {
            for n in names.iter().flatten() {
                slots.insert(n.clone());
            }
        }
    }
    for sys in &parsed.systems {
        slots.extend(sys.assigns.keys().cloned());
        slots.extend(sys.update.keys().cloned());
    }
    // 3. Rewrite implicit-integration rule lines to `inte slot = rate`.
    let (step2, rules) = rewrite_rules(&step1, &slots);
    // 4. Mark the source as v0.3.
    let (source, _) = ensure_version(&step2);
    Ok(Migration {
        source,
        rules_migrated: rules,
        already_current: false,
    })
}
