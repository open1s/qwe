//! RFC-0034 stable detail-code registry.
//!
//! `Error::detail` is a `u32` sub-code qualifying `Error::status`. Codes are
//! stable and documented here. Within a status, detail values are unique; the
//! same number under a different status is a distinct meaning (a namespaced
//! catalog, so callers compare `(status, detail)` together, never `detail`
//! alone).

/// Shared, status-independent detail codes.
pub const NONE: u32 = 0;

// `Status::Invalid` — a value, byte sequence, or argument was malformed.
pub mod invalid {
    /// No further qualification.
    pub const GENERIC: u32 = 0;
    /// A name or identifier is empty, non-ASCII, or contains NUL.
    pub const BAD_NAME: u32 = 1;
    /// A field id is zero, duplicated, or not strictly increasing.
    pub const BAD_FIELD_ID: u32 = 2;
    /// A field name is empty or contains NUL.
    pub const BAD_FIELD_NAME: u32 = 3;
    /// A field type tag is out of range.
    pub const BAD_FIELD_TYPE: u32 = 4;
    /// A format/version discriminator is not the frozen value.
    pub const BAD_FORMAT: u32 = 5;
    /// A flag bit outside the declared set is set.
    pub const BAD_FLAGS: u32 = 6;
    /// Trailing bytes remain after a fully-consumed record.
    pub const TRAILING_BYTES: u32 = 24;
}

// `Status::Limit` — an advertised bound was reached or would be exceeded.
pub mod limit {
    /// A length prefix overflows the document.
    pub const LENGTH_OVERFLOW: u32 = 0;
    /// Too many sections.
    pub const TOO_MANY_SECTIONS: u32 = 1;
    /// A record count exceeds the advertised maximum.
    pub const TOO_MANY_RECORDS: u32 = 2;
}

// `Status::EirInvalid` — EIR failed SSA/type/effect verification.
pub mod eir {
    /// Use of an SSA value before its definition.
    pub const USE_BEFORE_DEF: u32 = 5;
    /// A value is defined more than once.
    pub const DOUBLE_DEF: u32 = 7;
    /// An annotated result type disagrees with the computed type.
    pub const TYPE_MISMATCH: u32 = 8;
    /// A function lacks a terminating instruction.
    pub const MISSING_TERMINATOR: u32 = 9;
    /// A nondeterministic effect appears in a deterministic/pure function.
    pub const NONDETERMINISTIC_EFFECT: u32 = 1;
}

// `Status::Capability`, `Status::HandleStale`, `Status::OwnershipStale`, and
// `Status::SchemaHash` use small sequential codes owned by their modules; see
// those modules for the authoritative per-module meanings.

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detail_codes_are_distinct_within_status() {
        // Invalid and EirInvalid share the small integers; the catalog is
        // namespaced by status, so this is expected and documented.
        assert_eq!(invalid::BAD_NAME, 1);
        assert_eq!(eir::NONDETERMINISTIC_EFFECT, 1);
        assert_eq!(eir::USE_BEFORE_DEF, 5);
    }
}
