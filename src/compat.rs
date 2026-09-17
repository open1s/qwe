//! RFC-0018 version domains and compatibility matrix (folded into the frozen
//! contract).
//!
//! World schema, Component ABI, WIR, Domain IR, EIR, Runtime ABI, Plugin ABI,
//! network protocol, and resource schema are versioned independently. A runtime
//! advertises an accepted major/minor range per domain; an unknown major is
//! rejected (fail closed, never implicit downgrade).

use crate::Result;

/// A semantic version triple. Major breaks; minor adds compatibly; patch fixes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

/// An accepted-version advertisement for one domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionBound {
    pub major: u32,
    pub min_minor: u32,
    pub max_minor: u32,
}

impl VersionBound {
    /// Rejects an unknown major, and a minor outside the advertised range.
    pub fn accepts(self, version: SemVer) -> Result<()> {
        if version.major != self.major {
            return Err(crate::Error {
                status: crate::Status::AbiMismatch,
                detail: 2,
                byte_offset: 0,
            });
        }
        if version.minor < self.min_minor || version.minor > self.max_minor {
            return Err(crate::Error {
                status: crate::Status::AbiMismatch,
                detail: 3,
                byte_offset: 0,
            });
        }
        Ok(())
    }
}

/// The full compatibility matrix a runtime advertises across every versioned
/// domain. An implementation fills only the domains it supports; the rest are
/// `None`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompatibilityMatrix {
    pub world_schema: Option<VersionBound>,
    pub component_abi: Option<VersionBound>,
    pub wir: Option<VersionBound>,
    pub domain_ir: Option<VersionBound>,
    pub eir: Option<VersionBound>,
    pub runtime_abi: Option<VersionBound>,
    pub plugin_abi: Option<VersionBound>,
    pub network_protocol: Option<VersionBound>,
    pub resource_schema: Option<VersionBound>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_bound_rejects_unknown_major_and_minor() {
        let bound = VersionBound {
            major: 2,
            min_minor: 0,
            max_minor: 5,
        };
        assert!(bound.accepts(SemVer::new(2, 3, 0)).is_ok());
        assert_eq!(
            bound.accepts(SemVer::new(3, 0, 0)).unwrap_err().status,
            crate::Status::AbiMismatch
        );
        assert_eq!(
            bound.accepts(SemVer::new(2, 9, 0)).unwrap_err().status,
            crate::Status::AbiMismatch
        );
    }
}
