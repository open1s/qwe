//! Full-pipeline demo: a physics world travels the entire documented compile
//! stack and runs end to end.
//!
//!   World (dsl WorldModel)
//!     → WIR       (RFC-0020 portable serialization, lower_to_wir)
//!     → Domain IR (RFC-0032 physics pipeline, hashed + lowered)
//!     → EIR       (RFC-0021 typed SSA, validated + dominance-gated)
//!     → Runtime   (interpreter == JIT == AOT agreement on a live scene)
//!
//! Run: `cargo run --example pipeline_demo`

use pwe_api::{EntityId, WorldId, WorldVersion};
use pwe_reference::aot::AotProgram;
use pwe_reference::domain_ir::DomainProgram;
use pwe_reference::dsl::WorldModel;
use pwe_reference::lang;
use pwe_reference::math::Vec3;
use pwe_reference::physics_ir::PhysicsNode;

fn main() -> pwe_api::Result<()> {
    // 1. World: a portable model (gravity + entities).
    let mut model = WorldModel::new(Vec3::new(0.0, -9.81, 0.0));
    model.entities.push(pwe_reference::dsl::EntityDecl {
        name: "vehicle".into(),
        position: Some(Vec3::new(0.0, 8.0, 0.0)),
        velocity: Some(Vec3::new(4.0, 0.0, 0.0)),
        mass: Some(4.0),
        dynamic: Some(true),
        restitution: Some(0.5),
        friction: None,
        collider: Some(pwe_reference::dsl::ColliderDecl::Box {
            dims: Vec3::new(1.0, 0.5, 0.7),
        }),
        camera: None,
        state: None,
        state_names: None,
        color: None,
        nbody: None,
    });
    model.entities.push(pwe_reference::dsl::EntityDecl {
        name: "ground".into(),
        position: Some(Vec3::new(0.0, -5.0, 0.0)),
        velocity: None,
        mass: None,
        dynamic: Some(false),
        restitution: None,
        friction: None,
        collider: Some(pwe_reference::dsl::ColliderDecl::Box {
            dims: Vec3::new(50.0, 5.0, 50.0),
        }),
        camera: None,
        state: None,
        state_names: None,
        color: None,
        nbody: None,
    });
    println!("== 1. world (dsl WorldModel) ==");
    println!(
        "   {} entities, gravity {:?}",
        model.entities.len(),
        model.gravity
    );

    // 2. WIR: portable RFC-0020 serialization round-trip.
    let wir = model.lower_to_wir()?;
    let wir_bytes = wir.encode()?;
    let wir_rt = pwe_reference::wir::WirDocument::decode(&wir_bytes)?;
    println!("\n== 2. WIR (RFC-0020) ==");
    println!(
        "   {} bytes, {} entity records round-trip",
        wir_bytes.len(),
        wir_rt.entities.len()
    );

    // 3. Domain IR: the RFC-0032 physics pipeline, hashed and lowered to EIR.
    let pipeline = PhysicsNode::pipeline(
        7,
        vec![pwe_reference::physics_eir::transform_id()],
        vec![pwe_reference::physics_eir::transform_id()],
    );
    let mut program = DomainProgram::default();
    for node in pipeline.values() {
        program.nodes.push(node.node.clone());
    }
    let domain_hash = program.hash()?;
    let (lowered, lowered_hash) = program.lower()?;
    println!("\n== 3. domain IR (RFC-0032 physics pipeline) ==");
    println!(
        "   {} pipeline nodes, domain hash determinism: {}",
        program.nodes.len(),
        program.hash()? == domain_hash
    );
    println!(
        "   lowered {} EIR functions (domain_hash == lowered hash: {})",
        lowered.functions.len(),
        domain_hash == lowered_hash
    );

    // 4. EIR: validate + RFC-0021 dominance gate + binary round-trip.
    lowered.validate(true)?;
    lowered.verify_linear_dominance()?;
    let eir_bytes = lowered.encode()?;
    let eir_rt = pwe_reference::eir::EirModule::decode(&eir_bytes)?;
    println!("\n== 4. EIR (RFC-0021) ==");
    println!(
        "   valid + dominance-gated, {} bytes round-trip, {} functions",
        eir_bytes.len(),
        eir_rt.functions.len()
    );

    // 5. Runtime: compile the language version and run interpreter==JIT==AOT.
    println!("\n== 5. runtime: interpreter == JIT == AOT ==");
    let source = r#"
        world {
            gravity = (0, -9.81, 0)
            entity v { position = (0, 8, 0); velocity = (4, 0, 0); mass = 4; dynamic = true; box = (1, 0.5, 0.7) }
            entity g { position = (0, -5, 0); dynamic = false; box = (50, 5, 50) }
        }
        systems { gravity { gravity_y = -9.81; dt = 1 / 60 } integrate { dt = 1 / 60 } ground_contact { restitution = 0.5 } }
    "#;
    let compiled = lang::compile(source)?;
    let aot = AotProgram::compile(&compiled.eir, 0)?;
    println!("   AOT artifact hash: {:?}", aot.artifact_hash);
    println!(
        "   AOT == interpreter (same EIR): {}",
        aot.execute(WorldId(1), WorldVersion(0))?
            == compiled.eir.interpret(WorldId(1), WorldVersion(0))?
    );

    let mut rt = lang::LangRuntime::compile(source)?;
    for _ in 0..120 {
        rt.step_cross()?; // enforces interpreter == JIT each tick
    }
    let y = rt.scene.position(EntityId(1))?.y;
    println!("   120 cross-backend steps, vehicle settled y = {y:.3}");

    println!("\nPipeline complete: World → WIR → Domain IR → EIR → Runtime.");
    Ok(())
}
