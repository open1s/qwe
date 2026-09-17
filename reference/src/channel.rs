//! Go-like channels with cross-runtime, cross-platform communication.
//!
//! A [`Channel`] is a bounded FIFO of byte messages with `send`/`recv`
//! (non-blocking, deterministic), like a Go buffered channel without the
//! goroutine/blocking semantics. Because PWE's determinism is first-class, the
//! primitives here are explicit: `try_send` reports a full buffer, `try_recv`
//! reports an empty one.
//!
//! Channels are also **networked**: a message is addressed to a
//! [`ChannelAddr`] `(region, channel)` and, when the destination is a different
//! runtime/node, it is serialized to the portable [`crate::wire`] byte format
//! and routed through a [`ChannelRouter`]. The wire bytes are the transport —
//! any runtime/platform that can decode them can exchange messages, so two
//! independent runtimes (different processes/hosts) can communicate.

use crate::wire::{Reader, Writer};
use pwe_api::{Error, RegionId, Result, Status};
use std::collections::{BTreeMap, VecDeque};

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// Identifies a channel within a runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChannelId(pub u64);

/// A globally addressable channel: a `(region, channel)` pair. `region` is the
/// runtime/node that owns the channel; `channel` identifies it there.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChannelAddr {
    pub region: RegionId,
    pub channel: ChannelId,
}

impl ChannelAddr {
    pub fn new(region: RegionId, channel: ChannelId) -> Self {
        Self { region, channel }
    }
}

/// A bounded Go-like channel of byte messages. Order is FIFO; `send` on a full
/// buffer reports `Status::Limit`, `recv` on an empty one returns `None`.
#[derive(Clone, Debug)]
pub struct Channel {
    pub id: ChannelId,
    pub capacity: usize,
    buffer: VecDeque<Vec<u8>>,
}

impl Channel {
    pub fn new(id: ChannelId, capacity: usize) -> Self {
        Self {
            id,
            capacity,
            buffer: VecDeque::with_capacity(capacity),
        }
    }

    /// Sends `msg`, buffering it. `Err(Status::Limit)` if the buffer is full
    /// (non-blocking, deterministic — no goroutine waits here).
    pub fn send(&mut self, msg: &[u8]) -> Result<()> {
        if self.buffer.len() >= self.capacity {
            return Err(error(Status::Limit, 1));
        }
        self.buffer.push_back(msg.to_vec());
        Ok(())
    }

    /// Receives the oldest message, or `Ok(None)` if the buffer is empty.
    pub fn recv(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(self.buffer.pop_front())
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// A networked message: a payload addressed to a `(region, channel)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkMessage {
    pub to: ChannelAddr,
    pub payload: Vec<u8>,
}

impl NetworkMessage {
    /// Serializes the message to the portable wire format (the transport).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.u64(self.to.region.0)?;
        out.u64(self.to.channel.0)?;
        out.bytes(&self.payload)?;
        Ok(out.finish())
    }

    /// Decodes a message from the wire format.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        let region = RegionId(input.u64()?);
        let channel = ChannelId(input.u64()?);
        let payload = input.bytes()?.to_vec();
        input.finish()?;
        Ok(Self {
            to: ChannelAddr::new(region, channel),
            payload,
        })
    }
}

/// A per-runtime router of channels. Messages sent to a local `region` are
/// delivered to the local channel; messages addressed to another region are
/// buffered in an outbox and transported via the wire format.
#[derive(Clone, Debug)]
pub struct ChannelRouter {
    /// The region this runtime owns.
    pub region: RegionId,
    channels: BTreeMap<ChannelAddr, Channel>,
    outbox: Vec<NetworkMessage>,
}

impl ChannelRouter {
    pub fn new(region: RegionId) -> Self {
        Self {
            region,
            channels: BTreeMap::new(),
            outbox: Vec::new(),
        }
    }

    /// Gets or creates the channel at `addr` with the given capacity.
    pub fn channel(&mut self, addr: ChannelAddr, capacity: usize) -> &mut Channel {
        self.channels
            .entry(addr)
            .or_insert_with(|| Channel::new(addr.channel, capacity))
    }

    /// Sends `payload` to `to`. A local channel must exist (created with
    /// [`ChannelRouter::channel`]); remote messages are queued for transport.
    pub fn send(&mut self, to: ChannelAddr, payload: &[u8]) -> Result<()> {
        if to.region == self.region {
            let ch = self
                .channels
                .get_mut(&to)
                .ok_or(error(Status::HandleStale, 1))?;
            ch.send(payload)
        } else {
            self.outbox.push(NetworkMessage {
                to,
                payload: payload.to_vec(),
            });
            Ok(())
        }
    }

    /// Receives from a local channel that must exist and belong to this runtime.
    pub fn recv(&mut self, from: ChannelAddr) -> Result<Option<Vec<u8>>> {
        if from.region != self.region {
            return Err(error(Status::HandleStale, 2));
        }
        let ch = self
            .channels
            .get_mut(&from)
            .ok_or(error(Status::HandleStale, 1))?;
        ch.recv()
    }

    /// Number of messages queued for network transport.
    pub fn outbox_len(&self) -> usize {
        self.outbox.len()
    }

    /// Serializes every pending outbound message into one wire document (the
    /// bytes that would be sent over the network to another runtime).
    pub fn encode_outbox(&self) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.u32(self.outbox.len() as u32)?;
        for m in &self.outbox {
            let enc = m.encode()?;
            out.bytes(&enc)?;
        }
        Ok(out.finish())
    }

    /// Ingests a wire document produced by another runtime's `encode_outbox`,
    /// delivering each message to its local channel (or re-queueing if still
    /// remote). Returns the number of messages delivered locally.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<usize> {
        let mut input = Reader::new(bytes)?;
        let count = input.u32()? as usize;
        let mut delivered = 0usize;
        for _ in 0..count {
            let msg = NetworkMessage::decode(input.bytes()?)?;
            if msg.to.region == self.region {
                let ch = self
                    .channels
                    .get_mut(&msg.to)
                    .ok_or(error(Status::HandleStale, 1))?;
                ch.send(&msg.payload)?;
                delivered += 1;
            } else {
                // Not this runtime's: re-queue for onward transport.
                self.outbox.push(msg);
            }
        }
        Ok(delivered)
    }

    /// A deterministic content identity over every local channel's messages
    /// (replay / cross-runtime agreement).
    pub fn hash(&self) -> pwe_api::Hash256 {
        let mut all = Vec::new();
        all.extend_from_slice(&self.region.0.to_le_bytes());
        for (addr, ch) in &self.channels {
            all.extend_from_slice(&addr.region.0.to_le_bytes());
            all.extend_from_slice(&addr.channel.0.to_le_bytes());
            all.extend_from_slice(&(ch.len() as u64).to_le_bytes());
            for msg in &ch.buffer {
                all.extend_from_slice(msg);
            }
        }
        crate::sha256::digest(&all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_is_bounded_fifo() {
        let mut c = Channel::new(ChannelId(1), 2);
        assert!(c.send(b"a").is_ok());
        assert!(c.send(b"b").is_ok());
        // Full: a third send is rejected.
        assert_eq!(c.send(b"c").unwrap_err().status, Status::Limit);
        // FIFO order on receive.
        assert_eq!(c.recv().unwrap().unwrap(), b"a");
        assert_eq!(c.recv().unwrap().unwrap(), b"b");
        assert!(c.recv().unwrap().is_none());
    }

    #[test]
    fn network_message_round_trips_wire_format() {
        let msg = NetworkMessage {
            to: ChannelAddr::new(RegionId(7), ChannelId(42)),
            payload: vec![1, 2, 3, 4, 5],
        };
        let enc = msg.encode().unwrap();
        let dec = NetworkMessage::decode(&enc).unwrap();
        assert_eq!(dec, msg);
    }

    #[test]
    fn local_channels_route_within_a_runtime() {
        let mut router = ChannelRouter::new(RegionId(1));
        let addr = ChannelAddr::new(RegionId(1), ChannelId(9));
        router.channel(addr, 4); // explicit buffered channel
        router.send(addr, b"hello").unwrap();
        router.send(addr, b"world").unwrap();
        assert_eq!(router.recv(addr).unwrap().unwrap(), b"hello");
        assert_eq!(router.recv(addr).unwrap().unwrap(), b"world");
        assert!(router.recv(addr).unwrap().is_none());
    }

    #[test]
    fn cross_runtime_messages_travel_over_the_wire() {
        // Two independent runtimes (different regions) exchange messages through
        // the wire transport, exactly as two processes/hosts would.
        let mut runtime_a = ChannelRouter::new(RegionId(1));
        let mut runtime_b = ChannelRouter::new(RegionId(2));
        let a_to_b = ChannelAddr::new(RegionId(2), ChannelId(5));
        let b_to_a = ChannelAddr::new(RegionId(1), ChannelId(5));
        runtime_a.channel(b_to_a, 4);
        runtime_b.channel(a_to_b, 4);

        // A sends to B's channel; it queues in A's outbox (networked).
        runtime_a.send(a_to_b, b"ping from A").unwrap();
        assert_eq!(runtime_a.outbox_len(), 1);

        // The wire document is transported from A to B.
        let transport = runtime_a.encode_outbox().unwrap();
        let delivered = runtime_b.ingest(&transport).unwrap();
        assert_eq!(delivered, 1);

        // B receives A's message.
        assert_eq!(runtime_b.recv(a_to_b).unwrap().unwrap(), b"ping from A");

        // And the reverse direction.
        runtime_b.send(b_to_a, b"pong from B").unwrap();
        let transport = runtime_b.encode_outbox().unwrap();
        runtime_a.ingest(&transport).unwrap();
        assert_eq!(runtime_a.recv(b_to_a).unwrap().unwrap(), b"pong from B");
    }

    #[test]
    fn routers_agree_on_identical_exchanges() {
        // Two routers performing the same exchange end with identical state.
        let mk = |seed: u8| {
            let mut r = ChannelRouter::new(RegionId(1));
            let addr = ChannelAddr::new(RegionId(1), ChannelId(3));
            r.channel(addr, 4);
            r.send(addr, &[seed, 1, 2]).unwrap();
            r.send(addr, &[seed, 3, 4]).unwrap();
            r
        };
        assert_eq!(mk(7).hash(), mk(7).hash());
        assert_ne!(mk(7).hash(), mk(8).hash());
    }
}
