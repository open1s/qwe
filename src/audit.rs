//! RFC-0017 security-audit model (folded into the frozen contract).
//!
//! Authority is explicit: possessing a pointer or handle does not imply
//! permission. Every security-sensitive mutation records an audit event
//! carrying actor/module identity, the capability used, the WorldVersion, and
//! the operation performed.

use crate::{Hash256, RegionId, WorldVersion};

/// Identifies the actor or module that performed a mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Actor {
    /// An in-process system or module identified by a stable hash.
    System(Hash256),
    /// A remote region acting through the distributed protocol.
    RemoteRegion(RegionId),
    /// An external untrusted actor via a capability token issuer.
    External(Hash256),
}

/// The kind of security-sensitive operation being audited.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AuditOperation {
    ComponentWrite = 0,
    EntityCreate = 1,
    EntityDestroy = 2,
    OwnershipTransfer = 3,
    ResourceWrite = 4,
    NetworkSend = 5,
    JitInstall = 6,
}

/// A single audit record of a privileged mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditEvent {
    pub actor: Actor,
    /// The capability token identity used (contents, not the raw token).
    pub capability: Hash256,
    /// The authoritative world version the operation applied to.
    pub world_version: WorldVersion,
    pub operation: AuditOperation,
    /// Location of the effect (entity, resource, or region).
    pub subject: Hash256,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_event_carries_actor_and_operation() {
        let event = AuditEvent {
            actor: Actor::System(Hash256([1; 32])),
            capability: Hash256([2; 32]),
            world_version: WorldVersion(7),
            operation: AuditOperation::OwnershipTransfer,
            subject: Hash256([3; 32]),
        };
        assert_eq!(event.world_version, WorldVersion(7));
        assert_eq!(event.operation, AuditOperation::OwnershipTransfer);
    }
}
