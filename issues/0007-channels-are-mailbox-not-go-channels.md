# 0007 — Channels are a last-writer-wins mailbox, not Go-style channels (Medium)

## Summary

`chan` + `send`/`recv` is marketed as "Go-style channel send/receive". The
implementation is a single-cell register: every sender overwrites the same
`state[0]`, the last writer in system order wins, nobody blocks, no queue
exists. The user-visible semantics of ordering and latency are unspecified.

## Evidence

- Implementation contract: `reference/src/lang/mod.rs:3206-3214` — "A channel
  is an entity … holding its latest value in `state[0]`. `send` … **the last
  sender wins**. `recv` … reads the channel's value into its own `slot`."
  Reads/writes are plain cross-entity component ops (`mod.rs:3262-3345`).
- Ordering is program text order: systems execute in declaration order; a
  `recv` declared before its `send` sees the *previous* step's value, after it
  sees this step's — a one-step latency flip driven by line order.
- Overstated naming: README features list and wiki call it "Go-style channel
  send/receive", implying queues, blocking, and multi-value semantics.

## Impact

Users will design protocols around blocking/queueing semantics that do not
exist (e.g. expecting two sends to both be received), and the line-order latency
flip is an invisible determinism/ordering trap — the language's own advice says
"system order matters" (`docs/lang-usage.md:463`), but nothing warns here.
Distributed runs make this worse: `ChannelRouter`/region ids
(`reference/src/channel.rs`, `mod.rs:40`) put the same mailbox across regions,
where "last writer" depends on transport arrival order unless the runtime
orders it by entity id — that ordering must be a stated contract.

## Fix

1. Rename the concept in docs (signal/bus/mailbox) or implement the missing
   Go-like parts (queue, blocking/rendezvous) — do not keep the middle ground.
2. Specify in the language reference: multi-writer resolution, whether `recv`
   observes same-step writes (it does when `send` is declared first), and the
   cross-region ordering rule (must be entity-id/deterministic, never arrival
   order).
3. Add a lint for a `recv` whose `send` counterpart is declared on the other
   side of it, surfacing the latency flip.

## Labels

language-design, distributed, determinism, medium
