//! Map key type: PHP `int|string`, kept disjoint (no PHP key coercion).
//!
//! `Shared\Map` accepts `int` and `string` keys and keeps them
//! **distinct** — `123` (an `Int`) and `"123"` (a `Str`) are two
//! different entries, unlike a native PHP array which coerces numeric
//! string keys to ints. The `Hash`/`Eq` impls tag the discriminant so
//! the two variants never collide in the DashMap bucket space.
//!
//! `Str` keys are binary-safe: they hold opaque bytes (`Arc<[u8]>`), so a
//! non-UTF-8 PHP string key round-trips faithfully (like a PHP array key,
//! a Go map key, or a Redis key) instead of being rejected. Wrapping
//! `Arc<[u8]>` also keeps cloning a key (e.g. snapshotting a shard's keys
//! for `forEach`) a refcount bump, not a buffer copy.

use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum MapKey {
    Int(i64),
    Str(Arc<[u8]>),
}

impl MapKey {
    /// Build a `Str` key from a `&str` (allocates a fresh `Arc<[u8]>`).
    pub fn from_str(s: &str) -> Self {
        MapKey::Str(Arc::from(s.as_bytes()))
    }

    /// Build a `Str` key from raw bytes (binary-safe; no UTF-8 check).
    pub fn from_bytes(b: &[u8]) -> Self {
        MapKey::Str(Arc::from(b))
    }
}

impl PartialEq for MapKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MapKey::Int(a), MapKey::Int(b)) => a == b,
            (MapKey::Str(a), MapKey::Str(b)) => a.as_ref() == b.as_ref(),
            // Int and Str are disjoint: Int(123) != Str("123").
            _ => false,
        }
    }
}
impl Eq for MapKey {}

impl std::hash::Hash for MapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Tag the discriminant so Int(123) and Str("123") never collide.
        match self {
            MapKey::Int(i) => {
                state.write_u8(0);
                i.hash(state);
            }
            MapKey::Str(s) => {
                state.write_u8(1);
                s.as_ref().hash(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn int_and_string_keys_are_distinct() {
        let mut m: HashMap<MapKey, i32> = HashMap::new();
        m.insert(MapKey::Int(123), 1);
        m.insert(MapKey::from_str("123"), 2);
        assert_eq!(m.len(), 2, "123 and \"123\" must be different keys");
        assert_eq!(m[&MapKey::Int(123)], 1);
        assert_eq!(m[&MapKey::from_str("123")], 2);
    }

    #[test]
    fn string_keys_equal_by_content() {
        assert_eq!(MapKey::from_str("a"), MapKey::from_str("a"));
        assert_ne!(MapKey::from_str("a"), MapKey::from_str("b"));
    }
}
