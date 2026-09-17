//! Distributed continuum demo: a chemistry diffusion field spread across two
//! cluster nodes, evolved by the BEAM-like scheduler, then one node's field
//! migrates to a third node through the RFC-0024 ownership machine. The global
//! state identity before and after migration is printed, proving the migrated
//! field resumes identically.
//!
//! Run: `cargo run -p pwe-reference --example distributed_demo`

use pwe_api::RegionId;
use pwe_reference::distributed::DistributedContinuum;
use pwe_reference::field::Field;

fn main() {
    let mut dc = DistributedContinuum::new(100, 1e-3, 0.5);

    // Node 1: a concentration spike; Node 2: two offset peaks.
    let mut f1 = Field::new(32, 1, 0.1);
    f1.set(8, 0, 4.0);
    let mut f2 = Field::new(32, 1, 0.1);
    f2.set(10, 0, 3.0);
    f2.set(22, 0, 2.0);
    dc.add_node(RegionId(1), f1);
    dc.add_node(RegionId(2), f2);

    println!("global_hash before = {:?}", dc.global_hash());
    for _ in 0..40 {
        dc.step();
    }
    println!("global_hash after evolution  = {:?}", dc.global_hash());
    println!(
        "node2 field hash pre-migration = {:?}",
        dc.field(RegionId(2)).unwrap().hash()
    );

    // Migrate node 2's field to node 3.
    dc.migrate_field(RegionId(2), RegionId(3)).unwrap();

    println!(
        "node2 field hash post-migration = {:?}",
        dc.field(RegionId(2)).map(|f| f.hash())
    );
    println!(
        "node3 field hash post-migration = {:?}",
        dc.field(RegionId(3)).unwrap().hash()
    );
    println!(
        "ownership of migrated entity on node3 = {:?}",
        dc.cluster
            .ownership
            .region(RegionId(3))
            .owner_of(pwe_reference::distributed::field_entity(RegionId(2)))
            .unwrap()
            .epoch
    );
}
