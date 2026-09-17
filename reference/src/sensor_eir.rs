//! Sensor domain node on the EIR layer (RFC-0007/0008 peer domain).
//!
//! A sensor measures another entity's component state and writes a readout into
//! its own sensor component. Like physics, this is lowered to typed EIR and run
//! by the interpreter — sensor behavior is expressed as IR, not host code.

use crate::eir::{ComponentRef, Instruction, Opcode, ValueType};
use crate::physics_eir::{field, instr, transform_id, EirSystem};
use crate::schema::ComponentIdentity;
use crate::sha256::digest;
use pwe_api::{ComponentTypeId, Hash256};

/// Stable component id for a sensor readout (RFC-0019 canonical).
pub fn sensor_readout_id() -> ComponentTypeId {
    let identity = ComponentIdentity {
        namespace: "pwe.sensor".into(),
        stable_name: "readout".into(),
        major_version: 1,
    };
    identity.type_id().expect("fixed valid component identity")
}

/// Field offsets within a sensor readout component.
pub mod sensor_field {
    /// Measured target y position (f64).
    pub const MEASURED_Y: u32 = 8;
    /// Target entity id (u64, low 64 bits).
    pub const TARGET_ID: u32 = 0;
}

/// A sensor system: reads `target`'s Transform.position.y and writes it into
/// the sensor entity's readout. Compose with physics systems in a
/// `PhysicsProgram`.
pub struct SensorSystem {
    /// The sensing entity (writes its readout).
    pub sensor: u128,
    /// The entity being measured.
    pub target: u128,
}

impl EirSystem for SensorSystem {
    fn name(&self) -> &'static str {
        "sensor.measure_y"
    }
    fn lower_entity(&self, entity: u128, out: &mut Vec<Instruction>) {
        // Only the sensor entity produces the readout; other entities are
        // skipped (no instructions).
        if entity != self.sensor {
            return;
        }
        // read target.pos.y
        out.push(instr(
            Opcode::ReadView,
            1,
            Some(ValueType::F64),
            vec![],
            None,
            Some(ComponentRef {
                entity: self.target,
                component: transform_id(),
                offset: field::POS_Y,
            }),
        ));
        // write sensor.readout.measured_y = measured
        out.push(instr(
            Opcode::WriteView,
            0,
            None,
            vec![1],
            None,
            Some(ComponentRef {
                entity: self.sensor,
                component: sensor_readout_id(),
                offset: sensor_field::MEASURED_Y,
            }),
        ));
        out.push(instr(Opcode::Return, 0, None, vec![], None, None));
    }
}

/// Canonical sensor readout hash for telemetry identity.
pub fn readout_hash(sensor: u128, measured_y: f64) -> Hash256 {
    let mut input = b"pwe.sensor/v2\0".to_vec();
    input.extend_from_slice(&sensor.to_le_bytes());
    input.extend_from_slice(&measured_y.to_bits().to_le_bytes());
    digest(&input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Transform, Velocity};
    use crate::math::Vec3;
    use crate::physics_eir::{PhysicsProgram, SceneRuntime};
    use crate::scene::{Entity, Scene};
    use pwe_api::EntityId;

    #[test]
    fn sensor_measures_target_position_through_eir() {
        let mut scene = Scene::new(Vec3::new(0.0, 0.0, 0.0));
        let mut target = Entity::dynamic();
        target.transform = Some(Transform {
            position: Vec3::new(0.0, 12.0, 0.0),
            ..Default::default()
        });
        target.velocity = Some(Velocity::default());
        scene.insert(EntityId(1), target);

        let mut sensor = Entity::dynamic();
        sensor.velocity = Some(Velocity::default());
        scene.insert(EntityId(2), sensor);

        let program = PhysicsProgram::build(
            vec![Box::new(SensorSystem {
                sensor: 2,
                target: 1,
            })],
            vec![1, 2],
        );
        program.module.validate(true).unwrap();

        let mut rt = SceneRuntime::new(&scene);
        let writes = program
            .module
            .interpret_with(&mut rt, pwe_api::WorldId(0), pwe_api::WorldVersion(0))
            .unwrap();

        // Exactly one write: the sensor readout of the target's height.
        assert_eq!(writes.len(), 1);
        let w = &writes[0];
        assert_eq!(w.entity, 2);
        assert_eq!(w.component, sensor_readout_id());
        assert_eq!(w.offset, sensor_field::MEASURED_Y);
        assert!((f64::from_bits(w.value) - 12.0).abs() < 1e-9);

        // Stable readout identity.
        let h = readout_hash(2, 12.0);
        assert_eq!(h, readout_hash(2, 12.0));
        assert_ne!(h, readout_hash(2, 13.0));
    }
}
