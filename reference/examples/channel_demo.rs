//! Go-like channels across runtimes: two independent `ChannelRouter`s (different
//! regions, e.g. two processes/hosts) exchange messages through the portable
//! wire format — the same bytes that would travel over a network.
//!
//! Run: `cargo run --example channel_demo`

use pwe_api::RegionId;
use pwe_reference::channel::{ChannelAddr, ChannelId, ChannelRouter};

fn main() -> pwe_api::Result<()> {
    // Runtime A (region 1) and runtime B (region 2) — independent, cross-platform.
    let mut a = ChannelRouter::new(RegionId(1));
    let mut b = ChannelRouter::new(RegionId(2));
    let a_to_b = ChannelAddr::new(RegionId(2), ChannelId(7));
    let b_to_a = ChannelAddr::new(RegionId(1), ChannelId(7));
    a.channel(b_to_a, 8);
    b.channel(a_to_b, 8);

    println!("== cross-runtime channels (Go-like, over the wire) ==");

    // A → B
    for i in 0..3 {
        a.send(a_to_b, format!("ping {i}").as_bytes())?;
    }
    let transport = a.encode_outbox()?; // the network bytes
    let delivered = b.ingest(&transport)?;
    println!(
        "A -> B: {delivered} message(s) transported ({} bytes)",
        transport.len()
    );
    for _ in 0..3 {
        let msg = b.recv(a_to_b)?.expect("message");
        println!("  B received: {:?}", String::from_utf8_lossy(&msg));
    }

    // B → A
    b.send(b_to_a, b"hello from B")?;
    let transport = b.encode_outbox()?;
    a.ingest(&transport)?;
    let msg = a.recv(b_to_a)?.expect("message");
    println!("B -> A: {:?}", String::from_utf8_lossy(&msg));

    // Deterministic state identity across the exchange.
    println!("\nrouter A content hash = {:?}", a.hash());
    println!("router B content hash = {:?}", b.hash());
    Ok(())
}
