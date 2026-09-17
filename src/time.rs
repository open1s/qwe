//! RFC-0013 time and determinism models (folded into the frozen contract).
//!
//! Simulation must never read wall-clock time implicitly; wall-clock access is
//! an explicit external input. A world may contain multiple clock domains
//! (physics 1000 Hz, sensor 100 Hz, AI 20 Hz, render 60 Hz). Determinism is a
//! declared level, not an afterthought.

use crate::{Hash256, Result};

/// A point in simulation time: a monotonic tick plus a nanosecond offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SimTime {
    pub tick: u64,
    pub nanos: i64,
}

impl SimTime {
    pub const ZERO: Self = Self { tick: 0, nanos: 0 };
}

/// The clock domains a world may contain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ClockDomain {
    Real = 0,
    Simulation = 1,
    Replay = 2,
    Prediction = 3,
}

/// Declared determinism guarantees for a schedule or world.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DeterminismLevel {
    /// No reproducibility guarantee; unordered reductions allowed.
    BestEffort = 0,
    /// Reproducible given the same inputs on the same implementation.
    Reproducible = 1,
    /// Bit-for-bit identical across implementations for the same inputs,
    /// schema set, seeds, and message ordering.
    StrictDeterministic = 2,
}

/// A fixed simulation step used by deterministic domains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedStep {
    /// Nanoseconds per tick.
    pub dt_nanos: i64,
}

impl FixedStep {
    /// Frequency in Hz converted to a fixed nanosecond step, rejecting zero.
    pub fn from_hz(hz: u64) -> Result<Self> {
        if hz == 0 {
            return Err(crate::Error {
                status: crate::Status::Invalid,
                detail: 1,
                byte_offset: 0,
            });
        }
        Ok(FixedStep {
            dt_nanos: (1_000_000_000u64 / hz) as i64,
        })
    }
}

/// The canonical replay inputs (RFC-0013 §7): snapshot + schema set + input
/// stream + configuration. All are identified by content hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayInput {
    pub initial_snapshot: Hash256,
    pub schema_set: Hash256,
    pub input_stream: Hash256,
    pub configuration: Hash256,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_step_from_hz_is_exact_for_common_rates() {
        assert_eq!(FixedStep::from_hz(1000).unwrap().dt_nanos, 1_000_000);
        assert_eq!(FixedStep::from_hz(60).unwrap().dt_nanos, 16_666_666);
        assert!(FixedStep::from_hz(0).is_err());
    }
    #[test]
    fn replay_input_is_content_identified() {
        let r = ReplayInput {
            initial_snapshot: Hash256([1; 32]),
            schema_set: Hash256([2; 32]),
            input_stream: Hash256([3; 32]),
            configuration: Hash256([4; 32]),
        };
        assert_eq!(r.initial_snapshot, Hash256([1; 32]));
    }
}
