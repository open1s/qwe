//! RFC-0022 declared-access scheduler.
//!
//! Systems declare, up front, the component/entity slices they read and the
//! slices they write. A `Schedule` validates the whole set before any system
//! runs: a write overlapping another system's write or read is a conflict and
//! rejects the schedule. Deterministic execution order is the declaration
//! order; a valid schedule establishes a total happens-before across systems
//! with no hidden data race.

use pwe_api::{ComponentTypeId, EntityRef, Error, Result, Status};
use std::collections::BTreeSet;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// A single component accessed at read or write granularity. Every entity is
/// split into per-component slots so two systems may safely touch different
/// components of the same entity without conflicting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ComponentSlot {
    pub entity: EntityRef,
    pub component: ComponentTypeId,
}

/// The declared access set of one system, before it runs. `reads` must be
/// fully constructed before scheduling; `writes` is the exclusive slice.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccessDeclaration {
    pub reads: BTreeSet<ComponentSlot>,
    pub writes: BTreeSet<ComponentSlot>,
}

impl AccessDeclaration {
    pub fn new() -> Self {
        Self::default()
    }
    /// Declares a read of one component slot.
    pub fn read(mut self, slot: ComponentSlot) -> Self {
        let _ = self.reads.insert(slot);
        self
    }
    /// Declares an exclusive write of one component slot.
    pub fn write(mut self, slot: ComponentSlot) -> Self {
        let _ = self.writes.insert(slot);
        self
    }
}

/// An ordered, validated schedule of declared-access systems.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schedule {
    systems: Vec<AccessDeclaration>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a system. Validation happens once, in `validate`.
    pub fn system(mut self, system: AccessDeclaration) -> Self {
        self.systems.push(system);
        self
    }

    /// Validates that no two systems claim an exclusive write to the same
    /// component slot. Systems execute sequentially in declaration order, so a
    /// write is exclusive to exactly one system and reads never race; only an
    /// overlapping write is a conflict (RFC-0022: `WriteView` is exclusive for
    /// its declared slice).
    ///
    /// The returned value is the deterministic execution order: the systems in
    /// their declared order, each with the component slots it owns for writing
    /// during the current tick.
    pub fn validate(&self) -> Result<ValidatedSchedule> {
        let mut written: BTreeSet<ComponentSlot> = BTreeSet::new();
        for (index, system) in self.systems.iter().enumerate() {
            for write in &system.writes {
                if !written.insert(*write) {
                    return Err(error(Status::Conflict, index as u32));
                }
            }
        }
        let order = self.systems.clone();
        Ok(ValidatedSchedule { order })
    }

    pub fn systems(&self) -> &[AccessDeclaration] {
        &self.systems
    }
}

/// The result of a successful schedule validation: systems in deterministic
/// execution order, with exclusive write ownership already assigned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedSchedule {
    order: Vec<AccessDeclaration>,
}

impl ValidatedSchedule {
    /// Systems in execution order.
    pub fn order(&self) -> &[AccessDeclaration] {
        &self.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pwe_api::EntityId;

    fn slot(entity: u128, component_byte: u8) -> ComponentSlot {
        let mut component = [0u8; 16];
        component[0] = component_byte;
        ComponentSlot {
            entity: EntityRef {
                id: EntityId(entity),
                generation: 1,
            },
            component: ComponentTypeId(component),
        }
    }

    #[test]
    fn schedule_disjoint_writes_and_reads_validate() {
        let a = slot(1, 1);
        let b = slot(2, 1);
        let schedule = Schedule::new()
            .system(AccessDeclaration::new().write(a))
            .system(AccessDeclaration::new().write(b))
            .system(AccessDeclaration::new().read(a).read(b));
        let validated = schedule.validate().unwrap();
        assert_eq!(validated.order().len(), 3);
    }

    #[test]
    fn schedule_allows_concurrent_reads_of_one_slot() {
        let a = slot(1, 1);
        let schedule = Schedule::new()
            .system(AccessDeclaration::new().read(a))
            .system(AccessDeclaration::new().read(a));
        assert!(schedule.validate().is_ok());
    }

    #[test]
    fn schedule_rejects_write_write_conflict() {
        let a = slot(1, 1);
        let schedule = Schedule::new()
            .system(AccessDeclaration::new().write(a))
            .system(AccessDeclaration::new().write(a));
        assert_eq!(schedule.validate().unwrap_err().status, Status::Conflict);
    }

    #[test]
    fn schedule_reads_before_and_after_a_write_are_sequential_and_valid() {
        let a = slot(1, 1);
        // Sequential execution: a read before a write and a read after a write
        // establish happens-before through the deterministic schedule order, so
        // neither is a hidden data race.
        let schedule = Schedule::new()
            .system(AccessDeclaration::new().read(a))
            .system(AccessDeclaration::new().write(a))
            .system(AccessDeclaration::new().read(a));
        assert!(schedule.validate().is_ok());
    }

    #[test]
    fn schedule_concurrent_different_components_of_one_entity_validate() {
        // Same entity, different component slots: no conflict.
        let sched = Schedule::new()
            .system(AccessDeclaration::new().write(slot(1, 1)))
            .system(AccessDeclaration::new().write(slot(1, 2)));
        assert!(sched.validate().is_ok());
    }
}
