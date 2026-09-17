//! RFC-0022 memory ordering and explicit fences for CPU/GPU handoff.
//!
//! RFC-0022: atomics are Relaxed/Acquire/Release/AcqRel/SeqCst only, and "CPU/GPU
//! transitions require explicit fence plus ownership transfer." This module makes
//! that contract concrete:
//!
//! * [`MemoryOrder`] — the permitted atomics/ordering set (no weaker custom
//!   orders leak through the API).
//! * [`Fence`] — an explicit, tagged ordering barrier that must accompany a
//!   CPU↔GPU resource handoff.
//! * [`CpuGpuHandoff`] — a transfer that **fails closed** unless the caller
//!   supplies both an explicit fence of at least `Release`/`Acquire` strength
//!   **and** an ownership transfer (RFC-0024), preventing unsynchronized
//!   visibility across the device boundary.

use pwe_api::{Error, Result, Status};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// The only memory-ordering strengths RFC-0022 permits. A plain access or a
/// weaker custom order is rejected by the API surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum MemoryOrder {
    Relaxed = 0,
    Acquire = 1,
    Release = 2,
    AcqRel = 3,
    SeqCst = 4,
}

impl MemoryOrder {
    /// A fence strong enough to publish a CPU write before a GPU read
    /// (`Release`, `AcqRel`, or `SeqCst`).
    pub fn is_releasing(self) -> bool {
        matches!(
            self,
            MemoryOrder::Release | MemoryOrder::AcqRel | MemoryOrder::SeqCst
        )
    }
    /// A fence strong enough to consume a GPU write before a CPU read
    /// (`Acquire`, `AcqRel`, or `SeqCst`).
    pub fn is_acquiring(self) -> bool {
        matches!(
            self,
            MemoryOrder::Acquire | MemoryOrder::AcqRel | MemoryOrder::SeqCst
        )
    }
}

/// An explicit ordering barrier. `tag` identifies the resource/phase it guards
/// so a fence cannot be recycled for a different handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fence {
    pub order: MemoryOrder,
    pub tag: u64,
}

/// A CPU↔GPU resource handoff. It carries an optional ownership-transfer token;
/// neither the fence nor the token may be missing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuGpuHandoff {
    /// Resource being transferred (a distinct id per resource).
    pub resource: u64,
    /// The explicit fence guarding this transition.
    pub fence: Fence,
    /// The RFC-0024 ownership-transfer token; must match the fence tag.
    pub ownership_transfer: u64,
}

impl CpuGpuHandoff {
    /// Validates the handoff per RFC-0022: the fence must exist, be at least
    /// Release strength for a CPU→GPU direction (or Acquire for GPU→CPU), and
    /// be paired with an ownership transfer. Fails closed otherwise.
    pub fn validate(&self, direction: Direction) -> Result<()> {
        if !self.fence.order.is_releasing() && !self.fence.order.is_acquiring() {
            return Err(error(Status::Invalid, 1));
        }
        match direction {
            // Publishing CPU writes for a GPU consumer needs a releasing fence.
            Direction::CpuToGpu if !self.fence.order.is_releasing() => {
                return Err(error(Status::Invalid, 2));
            }
            // Consuming GPU writes on the CPU needs an acquiring fence.
            Direction::GpuToCpu if !self.fence.order.is_acquiring() => {
                return Err(error(Status::Invalid, 3));
            }
            _ => {}
        }
        // The ownership transfer must be present and bound to this resource.
        if self.ownership_transfer == 0 {
            return Err(error(Status::OwnershipStale, 4));
        }
        if self.ownership_transfer != self.fence.tag {
            return Err(error(Status::Conflict, 5));
        }
        Ok(())
    }
}

/// The direction of a device-boundary handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    CpuToGpu,
    GpuToCpu,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_to_gpu_requires_releasing_fence_and_transfer() {
        let ok = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::Release,
                tag: 42,
            },
            ownership_transfer: 42,
        };
        assert!(ok.validate(Direction::CpuToGpu).is_ok());

        // Relaxed is not strong enough to publish for the GPU.
        let weak = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::Relaxed,
                tag: 42,
            },
            ownership_transfer: 42,
        };
        assert_eq!(
            weak.validate(Direction::CpuToGpu).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn gpu_to_cpu_requires_acquiring_fence() {
        let ok = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::Acquire,
                tag: 9,
            },
            ownership_transfer: 9,
        };
        assert!(ok.validate(Direction::GpuToCpu).is_ok());

        // A Release fence cannot acquire the GPU's writes on the CPU.
        let wrong = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::Release,
                tag: 9,
            },
            ownership_transfer: 9,
        };
        assert_eq!(
            wrong.validate(Direction::GpuToCpu).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn handoff_without_ownership_transfer_fails_closed() {
        // Missing ownership transfer: rejected even with a strong fence.
        let no_transfer = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::SeqCst,
                tag: 1,
            },
            ownership_transfer: 0,
        };
        assert_eq!(
            no_transfer
                .validate(Direction::CpuToGpu)
                .unwrap_err()
                .status,
            Status::OwnershipStale
        );
        // Ownership transfer must match the fence tag (no recycling across
        // handoffs).
        let mismatched = CpuGpuHandoff {
            resource: 7,
            fence: Fence {
                order: MemoryOrder::SeqCst,
                tag: 1,
            },
            ownership_transfer: 2,
        };
        assert_eq!(
            mismatched.validate(Direction::CpuToGpu).unwrap_err().status,
            Status::Conflict
        );
    }

    #[test]
    fn fence_memory_orders_are_bounded_to_rfc_set() {
        // The only permitted orders; a hypothetical weaker custom order has no
        // representation here — the enum is closed.
        assert!(MemoryOrder::Relaxed < MemoryOrder::Acquire);
        assert!(MemoryOrder::Acquire < MemoryOrder::Release);
        assert!(MemoryOrder::Release < MemoryOrder::AcqRel);
        assert!(MemoryOrder::AcqRel < MemoryOrder::SeqCst);
    }
}
