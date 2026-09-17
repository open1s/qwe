# RFC-0006: Distributed World Protocol

**Status:** Draft \| **Version:** 0.1.0

## 1. Goal

Distribute one logical world over heterogeneous nodes without creating
multiple authorities.

## 2. Region

A Region is a scheduling/ownership unit. A node may host many Regions.

## 3. Ownership

At any instant, a mutable Entity has exactly one authoritative owner
Region.

## 4. Protocol objects

WorldCommand, WorldEvent, StateDelta, Snapshot, OwnershipTransfer,
InterestSubscription, SchemaAnnouncement.

## 5. Transport

Initial reference transport: QUIC. Transport is replaceable and is not
part of World semantics.

## 6. Ownership transfer

Prepare -\> Freeze -\> Transfer -\> Confirm -\> Resume. During transfer,
the old owner cannot commit new authoritative writes after the transfer
cutover.

## 7. Replication

Replication is schema-driven. Never copy arbitrary process memory.

## 8. Prediction and rollback

Clients may predict non-authoritative state. Authoritative corrections
carry WorldVersion/TimePoint and can trigger rollback/replay.

## 9. Cross-region interaction

Interactions crossing regions use explicit messages or replicated
boundary state. Deterministic simulations must define message ordering.

## 10. Partition policy

Network partition must resolve explicitly as Halt,
ContinueWithStaleRead, or AuthorityReassignment; split-brain authority
is forbidden.
