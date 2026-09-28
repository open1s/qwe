//! Deterministic, dependency-free fuzzing (P4.4).
//!
//! Feeds pseudo-random bytes/strings into every public decode/parse boundary
//! and asserts it **never panics** and keeps its documented invariants (parsers
//! return `Result`; the formatter is idempotent). This is a bounded, in-tree
//! fuzzer that runs in CI — no external `cargo-fuzz`/libFuzzer toolchain.
//!
//! For deeper UB checking, run under Miri (nightly):
//!   `cargo +nightly miri test -p pwe-reference --test fuzz`

use pwe_reference::channel::NetworkMessage;
use pwe_reference::eir::EirModule;
use pwe_reference::extension::{CompressionMetadata, ExtensionEnvelope};
use pwe_reference::lang;

/// xorshift64* — tiny, deterministic, seedable.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Fewer iterations under Miri (it is ~100x slower).
fn iters(full: usize) -> usize {
    if cfg!(miri) {
        full / 50
    } else {
        full
    }
}

#[test]
fn decoders_never_panic_on_random_bytes() {
    let known: &[u16] = &[1, 2, 3, 7];
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
    for _ in 0..iters(4000) {
        let n = rng.below(96);
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        let _ = EirModule::decode(&bytes);
        let _ = CompressionMetadata::decode(&bytes);
        let _ = ExtensionEnvelope::decode(&bytes, known);
        let _ = NetworkMessage::decode(&bytes);
    }
}

#[test]
fn parser_and_compiler_never_panic_on_random_source() {
    const ALPHABET: &[u8] =
        b"world{}()[]=,.+-*/%<>!&|#\"'\n\t 0123456789 systems entity state update on dt inte deriv funcs let if else return for in random at periodic vec3 struct chan field camera color [m] [m/s] s0 s7 ghost;:,..";
    // Several seeds and varied lengths exercise numeric literals, unit
    // annotations, `sN` slots, and long inputs.
    for seed in [
        0xdead_beef_cafe_f00d,
        0x1234_5678_9abc_def0,
        0x0f0f_0f0f_0f0f_0f0f,
    ] {
        let mut rng = Rng::new(seed);
        for _ in 0..iters(1500) {
            let n = rng.below(400);
            let s: String = (0..n)
                .map(|_| ALPHABET[rng.below(ALPHABET.len())] as char)
                .collect();
            // Parsing must return a Result, never panic.
            let _ = lang::parse(&s);
            let _ = lang::LangRuntime::compile(&s);
            // The formatter must never panic and must be idempotent.
            let once = lang::format_source(&s);
            let twice = lang::format_source(&once);
            assert_eq!(once, twice, "format not idempotent on {s:?}");
        }
    }
}
