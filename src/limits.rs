//! RFC-0034 advertised resource limits.
//!
//! Every implementation advertises the bounds it enforces. The values below are
//! the RFC-0034 defaults; an implementation MAY advertise lower bounds but MUST
//! NOT silently exceed its advertised limits. A misbehaving or poorly-sized
//! input past a bound fails with `Status::Limit`.

/// Advertised limits an implementation enforces at its external boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum size of any canonical document, snapshot, or module, in bytes.
    pub max_document_bytes: u64,
    /// Maximum number of sections in a WIR or EIR directory.
    pub max_sections: u32,
    /// Maximum nesting depth of schema/module structures.
    pub max_nesting: u32,
    /// Maximum number of records (entities, components, blocks, ops) in one
    /// section or function.
    pub max_records: u32,
}

/// RFC-0034 default limits.
pub const DEFAULT_LIMITS: Limits = Limits {
    max_document_bytes: 256 * 1024 * 1024,
    max_sections: 1024,
    max_nesting: 64,
    max_records: 10_000_000,
};

impl Default for Limits {
    fn default() -> Self {
        DEFAULT_LIMITS
    }
}

impl Limits {
    /// A lower advertised limit is legal; a higher one is not.
    pub const fn within_default(self) -> bool {
        self.max_document_bytes <= DEFAULT_LIMITS.max_document_bytes
            && self.max_sections <= DEFAULT_LIMITS.max_sections
            && self.max_nesting <= DEFAULT_LIMITS.max_nesting
            && self.max_records <= DEFAULT_LIMITS.max_records
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_limits_match_rfc_numbers() {
        assert_eq!(DEFAULT_LIMITS.max_document_bytes, 256 * 1024 * 1024);
        assert_eq!(DEFAULT_LIMITS.max_sections, 1024);
        assert_eq!(DEFAULT_LIMITS.max_nesting, 64);
        assert_eq!(DEFAULT_LIMITS.max_records, 10_000_000);
        assert!(DEFAULT_LIMITS.within_default());
    }
    #[test]
    fn stricter_limits_are_advertised_not_exceeded() {
        let stricter = Limits {
            max_records: 1_000,
            ..DEFAULT_LIMITS
        };
        assert!(stricter.within_default());
        let higher = Limits {
            max_document_bytes: DEFAULT_LIMITS.max_document_bytes * 2,
            ..DEFAULT_LIMITS
        };
        assert!(!higher.within_default());
    }
}
