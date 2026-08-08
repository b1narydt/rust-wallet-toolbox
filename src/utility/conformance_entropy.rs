//! Process-wide deterministic entropy override for conformance recording/replay.
//!
//! The funded BRC-100 conformance vectors (`conformance/vectors/wallet/brc100/
//! *-funded.json`) pin exact transaction bytes. A `createAction` run consumes
//! entropy for the storage `reference` and the change-output BRC-42 derivation
//! prefix/suffixes; those derivations decide the change locking scripts and so
//! the txid. Recording and offline replay must draw the *same* entropy stream
//! or byte-for-byte reproduction is impossible.
//!
//! When a seed is set, [`fill_random`] yields `SHA-256(seed_hash || counter)`
//! blocks — a fixed construction with no dependency on `rand`'s (version-
//! dependent) generator internals, so a vector recorded today replays
//! identically forever. When no seed is set (the default, and the only state
//! production code ever runs in), [`fill_random`] is the thread RNG.
//!
//! The seed is process-global and the stream is consumed in call order, so a
//! seeded section must run one wallet operation at a time.

use std::sync::Mutex;

use sha2::{Digest, Sha256};

struct SeededStream {
    seed_hash: [u8; 32],
    counter: u64,
}

static OVERRIDE: Mutex<Option<SeededStream>> = Mutex::new(None);

/// Begin drawing deterministic entropy derived from `seed`.
///
/// Conformance recording/replay only. Never set this in production: it makes
/// key-derivation prefixes predictable.
pub fn set_conformance_entropy(seed: &str) {
    let mut guard = OVERRIDE.lock().expect("entropy lock");
    *guard = Some(SeededStream {
        seed_hash: Sha256::digest(seed.as_bytes()).into(),
        counter: 0,
    });
}

/// Return to real (thread RNG) entropy.
pub fn clear_conformance_entropy() {
    let mut guard = OVERRIDE.lock().expect("entropy lock");
    *guard = None;
}

/// Fill `buf` from the seeded stream if one is set, else from the thread RNG.
pub fn fill_random(buf: &mut [u8]) {
    let mut guard = OVERRIDE.lock().expect("entropy lock");
    match guard.as_mut() {
        Some(stream) => {
            let mut filled = 0;
            while filled < buf.len() {
                let mut hasher = Sha256::new();
                hasher.update(stream.seed_hash);
                hasher.update(stream.counter.to_be_bytes());
                stream.counter += 1;
                let block = hasher.finalize();
                let n = (buf.len() - filled).min(block.len());
                buf[filled..filled + n].copy_from_slice(&block[..n]);
                filled += n;
            }
        }
        None => {
            use rand::RngCore;
            rand::thread_rng().fill_bytes(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_stream_is_deterministic_and_order_dependent() {
        set_conformance_entropy("vector-a");
        let mut a1 = [0u8; 12];
        let mut a2 = [0u8; 16];
        fill_random(&mut a1);
        fill_random(&mut a2);

        set_conformance_entropy("vector-a");
        let mut b1 = [0u8; 12];
        let mut b2 = [0u8; 16];
        fill_random(&mut b1);
        fill_random(&mut b2);

        assert_eq!(a1, b1);
        assert_eq!(a2, b2);

        set_conformance_entropy("vector-b");
        let mut c1 = [0u8; 12];
        fill_random(&mut c1);
        assert_ne!(a1, c1);

        clear_conformance_entropy();
        let mut d1 = [0u8; 12];
        let mut d2 = [0u8; 12];
        fill_random(&mut d1);
        fill_random(&mut d2);
        assert_ne!(d1, d2);
    }
}
