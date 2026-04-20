//! Shared hasher choice for internal hash-based data structures.
//!
//! `foldhash::quality::RandomState` replaces `std`'s SipHash1-3 in the
//! routing and file-cache LRU maps. Motivation:
//!
//! - **Speed**: 3-5× faster than SipHash1-3 on the 15-60-byte URI keys used
//!   by `route_cache` and `FileCache`. On the hot path the hash cost often
//!   exceeds the uncontended `parking_lot::Mutex` itself.
//! - **DoS resistance**: route_cache keys come from attacker-controlled URIs.
//!   `foldhash::quality` is HashDoS-resistant; `foldhash::fast` is not.
//! - **Random seed**: per-process random seed prevents cross-process
//!   collision prediction attacks.

pub type FastHasher = foldhash::quality::RandomState;
