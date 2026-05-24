//! SharedValue — the internal value type stored inside Shared\*
//! containers. Supports scalars plus Array + Shared nesting (used by
//! Map).
//!
//! Lifetime model for nested `Shared\*` references:
//! - `SharedValue::Shared(SharedRefOwned)` owns an `Arc<Entry>` via
//!   `entry_ptr` (an `Arc::into_raw` pointer). `Clone` increments the
//!   strong count; `Drop` reconstitutes the `Arc` and drops it. This
//!   makes lifetime automatic — moving a `SharedValue` into a
//!   container transfers the Arc with it; dropping the `SharedValue`
//!   releases the nested entry.
//! - `SharedRef { id, type_tag }` is a 16-byte `Copy` view used by
//!   walkers (cycle detection, observability/graph endpoints). It does
//!   not affect lifetime.
//! - The portbuf wire codec is split into a pure decoder
//!   (`portbuf_to_sv` → `SharedValueRaw`) and an explicit registry
//!   resolution step (`raw_to_owned(raw, &SharedRegistry) →
//!   SharedValue`). Callers do both steps in succession; the codec
//!   itself never touches the registry.

use std::sync::Arc;

use smallvec::SmallVec;

use crate::plugins::ox_shared::registry::{Entry, SharedId, SharedRegistry, SharedType};

/// Container payload. The lifetime-bearing form of a value; safe to
/// store inside `Shared\Map` / `Shared\Mutex` / `Shared\Once`.
#[derive(Clone, Debug)]
pub enum SharedValue {
    Null,
    Bool(bool),
    Long(i64),
    Double(f64),
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    Array(Arc<SharedArray>),
    Shared(SharedRefOwned),
}

#[derive(Clone, Debug, Default)]
pub struct SharedArray {
    pub int_keyed: Vec<SharedValue>,
    pub str_keyed: Vec<(Arc<str>, SharedValue)>,
}

/// Cheap, non-owning view of a nested `Shared\*` reference. Used by
/// walkers (cycle detection, observability/graph endpoint) and as the
/// `Shared` payload of [`SharedValueRaw`] before registry resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SharedRef {
    pub id: SharedId,
    pub type_tag: SharedType,
}

/// Owning handle to an `Entry`, embedded inside
/// `SharedValue::Shared`. Holds a strong `Arc<Entry>` reference as an
/// `Arc::into_raw` pointer so the nested entry stays alive for as
/// long as the enclosing `SharedValue` does.
///
/// `Clone` calls `Arc::increment_strong_count`; `Drop` reconstitutes
/// the `Arc` via `Arc::from_raw` and drops it. `id` and `type_tag` are
/// mirrored from the `Entry` so [`as_view`](Self::as_view) and the
/// portbuf serializer can avoid a pointer dereference.
pub struct SharedRefOwned {
    entry_ptr: *const Entry,
    pub id: SharedId,
    pub type_tag: SharedType,
}

impl SharedRefOwned {
    /// Consume a strong `Arc<Entry>` and produce an owning handle. The
    /// strong count is **not** bumped — this transfers the input ref
    /// into the new `SharedRefOwned`.
    pub fn from_arc(arc: Arc<Entry>) -> Self {
        let id = arc.id;
        let type_tag = arc.type_tag;
        let entry_ptr = Arc::into_raw(arc);
        Self {
            entry_ptr,
            id,
            type_tag,
        }
    }

    /// Cheap projection for walker buffers and observability.
    pub fn as_view(&self) -> SharedRef {
        SharedRef {
            id: self.id,
            type_tag: self.type_tag,
        }
    }
}

impl Clone for SharedRefOwned {
    fn clone(&self) -> Self {
        // SAFETY: `entry_ptr` was produced by `Arc::into_raw` and is
        // still alive (this `SharedRefOwned` holds one strong count).
        unsafe { Arc::increment_strong_count(self.entry_ptr) };
        Self {
            entry_ptr: self.entry_ptr,
            id: self.id,
            type_tag: self.type_tag,
        }
    }
}

impl Drop for SharedRefOwned {
    fn drop(&mut self) {
        // SAFETY: `entry_ptr` was produced by `Arc::into_raw` for the
        // strong ref this `SharedRefOwned` owns. Reconstituting and
        // dropping balances that `into_raw`.
        unsafe { drop(Arc::from_raw(self.entry_ptr)) };
    }
}

impl std::fmt::Debug for SharedRefOwned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRefOwned")
            .field("id", &self.id)
            .field("type_tag", &self.type_tag)
            .finish()
    }
}

// `SharedRefOwned` carries a raw pointer to an `Entry`, which is
// `Send + Sync` (its interior atomics + `Arc<dyn SharedInner: Send +
// Sync>` make it so). The pointer behaves like an `Arc<Entry>`.
unsafe impl Send for SharedRefOwned {}
unsafe impl Sync for SharedRefOwned {}

/// Push every `SharedRef` view reachable from `sv` into `out`.
/// Recurses into `SharedValue::Array`. Used by the Map cycle-check
/// to extract roots and by `MapInner::children` to expose outgoing
/// edges to the walker.
pub fn collect_shared_refs(sv: &SharedValue, out: &mut Vec<SharedRef>) {
    match sv {
        SharedValue::Shared(r) => out.push(r.as_view()),
        SharedValue::Array(a) => {
            for v in &a.int_keyed {
                collect_shared_refs(v, out);
            }
            for (_, v) in &a.str_keyed {
                collect_shared_refs(v, out);
            }
        }
        _ => {}
    }
}

impl SharedValue {
    /// Approximate byte-size for capacity accounting. Spec 09-capacity
    /// flags the ±10-30% drift vs mallinfo as documented.
    pub fn mem_bytes(&self) -> usize {
        match self {
            Self::Null | Self::Bool(_) => 1,
            Self::Long(_) | Self::Double(_) => 8,
            Self::String(s) => s.len() + 16,
            Self::Bytes(b) => b.len() + 16,
            Self::Array(a) => {
                let ints: usize = a.int_keyed.iter().map(|v| v.mem_bytes()).sum();
                let strs: usize = a
                    .str_keyed
                    .iter()
                    .map(|(k, v)| k.len() + 16 + v.mem_bytes())
                    .sum();
                ints + strs + 64
            }
            Self::Shared(_) => 16,
        }
    }
}

// ── Portbuf wire-format codec ────────────────────────────────────────────────
//
// Tag table (matches C-side `portbuf_ser_zval` / `portrd_deser_zval`):
//   0  = null/undef
//   1  = true
//   2  = false
//   3  = long     → i64 LE (8 bytes)
//   4  = double   → f64 LE (8 bytes)
//   5  = string   → u32 LE len + bytes
//   6  = array    → u32 LE count, then per-entry: key_type(u8) + key + recursive value
//          key_type 0 = index key → u64 LE index
//          key_type 1 = string key → u32 LE klen + bytes
//   7  = shared ref → u8 type_tag + u64 LE shared_id
//
// The codec is split into:
//   sv_to_portbuf(&SharedValue) -> Vec<u8>       (serialize from owned)
//   portbuf_to_sv(&[u8]) -> SharedValueRaw       (pure decode — no registry)
//   raw_to_owned(SharedValueRaw, &SharedRegistry) -> SharedValue
//                                                (explicit lookup step)
//
// Callers that need a lifetime-bearing `SharedValue` from raw bytes do
// both steps in succession.

/// Raw, lifetime-free output of `portbuf_to_sv`. Identical in shape to
/// [`SharedValue`] except that the `Shared` variant carries a
/// non-owning [`SharedRef`] view — no `Arc<Entry>` is produced. Convert
/// to `SharedValue` via [`raw_to_owned`].
#[derive(Clone, Debug)]
pub enum SharedValueRaw {
    Null,
    Bool(bool),
    Long(i64),
    Double(f64),
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    Array(Arc<SharedArrayRaw>),
    Shared(SharedRef),
}

#[derive(Clone, Debug, Default)]
pub struct SharedArrayRaw {
    pub int_keyed: Vec<SharedValueRaw>,
    pub str_keyed: Vec<(Arc<str>, SharedValueRaw)>,
}

/// Serialise a `SharedValue` into the portbuf wire format.
/// Byte-for-byte compatible with the C-side `portbuf_ser_zval`
/// so `oxphp_portable_deserialize` can decode the result into a zval.
pub fn sv_to_portbuf(sv: &SharedValue) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    sv_write(sv, &mut buf);
    buf
}

fn sv_write(sv: &SharedValue, b: &mut Vec<u8>) {
    match sv {
        SharedValue::Null => b.push(0),
        SharedValue::Bool(true) => b.push(1),
        SharedValue::Bool(false) => b.push(2),
        SharedValue::Long(v) => {
            b.push(3);
            b.extend_from_slice(&v.to_le_bytes());
        }
        SharedValue::Double(v) => {
            b.push(4);
            b.extend_from_slice(&v.to_le_bytes());
        }
        SharedValue::String(s) => {
            b.push(5);
            let bytes = s.as_bytes();
            b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            b.extend_from_slice(bytes);
        }
        SharedValue::Bytes(bs) => {
            // Same tag as String — portbuf wire format does not distinguish.
            b.push(5);
            b.extend_from_slice(&(bs.len() as u32).to_le_bytes());
            b.extend_from_slice(bs);
        }
        SharedValue::Array(a) => {
            b.push(6);
            let count = (a.int_keyed.len() + a.str_keyed.len()) as u32;
            b.extend_from_slice(&count.to_le_bytes());
            for (idx, v) in a.int_keyed.iter().enumerate() {
                b.push(0); // index key type
                b.extend_from_slice(&(idx as u64).to_le_bytes());
                sv_write(v, b);
            }
            for (k, v) in &a.str_keyed {
                b.push(1); // string key type
                b.extend_from_slice(&(k.len() as u32).to_le_bytes());
                b.extend_from_slice(k.as_bytes());
                sv_write(v, b);
            }
        }
        SharedValue::Shared(r) => {
            b.push(7);
            b.push(r.type_tag as u8);
            b.extend_from_slice(&r.id.to_le_bytes());
        }
    }
}

/// Deserialise a `SharedValueRaw` from the portbuf wire format.
/// Returns the first complete value in the buffer; extra bytes are
/// ignored. Pure: does not access the registry. Convert to an owning
/// `SharedValue` via [`raw_to_owned`].
pub fn portbuf_to_sv(
    buf: &[u8],
) -> Result<SharedValueRaw, crate::plugins::ox_shared::error::SharedError> {
    let mut pos = 0usize;
    sv_read(buf, &mut pos)
}

fn sv_read(
    buf: &[u8],
    pos: &mut usize,
) -> Result<SharedValueRaw, crate::plugins::ox_shared::error::SharedError> {
    use crate::plugins::ox_shared::error::SharedError;

    fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, SharedError> {
        if *pos >= buf.len() {
            return Err(SharedError::Generic);
        }
        let v = buf[*pos];
        *pos += 1;
        Ok(v)
    }
    fn read_u32_le(buf: &[u8], pos: &mut usize) -> Result<u32, SharedError> {
        if *pos + 4 > buf.len() {
            return Err(SharedError::Generic);
        }
        let v = u32::from_le_bytes(buf[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        Ok(v)
    }
    fn read_u64_le(buf: &[u8], pos: &mut usize) -> Result<u64, SharedError> {
        if *pos + 8 > buf.len() {
            return Err(SharedError::Generic);
        }
        let v = u64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        Ok(v)
    }
    fn read_i64_le(buf: &[u8], pos: &mut usize) -> Result<i64, SharedError> {
        Ok(read_u64_le(buf, pos)? as i64)
    }
    fn read_f64_le(buf: &[u8], pos: &mut usize) -> Result<f64, SharedError> {
        Ok(f64::from_bits(read_u64_le(buf, pos)?))
    }
    fn read_bytes<'a>(buf: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], SharedError> {
        if *pos + n > buf.len() {
            return Err(SharedError::Generic);
        }
        let slice = &buf[*pos..*pos + n];
        *pos += n;
        Ok(slice)
    }

    let tag = read_u8(buf, pos)?;
    Ok(match tag {
        0 => SharedValueRaw::Null,
        1 => SharedValueRaw::Bool(true),
        2 => SharedValueRaw::Bool(false),
        3 => SharedValueRaw::Long(read_i64_le(buf, pos)?),
        4 => SharedValueRaw::Double(read_f64_le(buf, pos)?),
        5 => {
            let len = read_u32_le(buf, pos)? as usize;
            let bytes = read_bytes(buf, pos, len)?;
            match std::str::from_utf8(bytes) {
                Ok(s) => SharedValueRaw::String(Arc::from(s)),
                Err(_) => SharedValueRaw::Bytes(Arc::from(bytes)),
            }
        }
        6 => {
            let count = read_u32_le(buf, pos)? as usize;
            let mut arr = SharedArrayRaw::default();
            for _ in 0..count {
                let key_type = read_u8(buf, pos)?;
                if key_type == 1 {
                    let klen = read_u32_le(buf, pos)? as usize;
                    let kbytes = read_bytes(buf, pos, klen)?;
                    let key = Arc::<str>::from(std::str::from_utf8(kbytes).unwrap_or(""));
                    let val = sv_read(buf, pos)?;
                    arr.str_keyed.push((key, val));
                } else {
                    let _idx = read_u64_le(buf, pos)?;
                    let val = sv_read(buf, pos)?;
                    arr.int_keyed.push(val);
                }
            }
            SharedValueRaw::Array(Arc::new(arr))
        }
        7 => {
            let tt_byte = read_u8(buf, pos)?;
            let id = read_u64_le(buf, pos)?;
            let type_tag = SharedType::from_tag(tt_byte).ok_or(SharedError::Type)?;
            SharedValueRaw::Shared(SharedRef { id, type_tag })
        }
        _ => return Err(SharedError::Type),
    })
}

/// Scan portbuf wire bytes for nested `Shared\*` references (tag-7) without
/// materializing scalars/strings/arrays. Walks exactly one value, skipping
/// scalar and string payloads by length, recursing into tag-6 arrays.
/// Returns the `SharedRef` views of every tag-7 found. Mirrors the tag
/// layout of `sv_read`; errors on a truncated or malformed buffer.
///
/// **Unbounded recursion** on array nesting, mirroring `sv_read` (Map's
/// decode path). A pathologically deep portbuf array would overflow the
/// stack here — but the same input overflows the C portbuf codec first:
/// `oxphp_portable_serialize` recursed to *produce* these bytes on send, and
/// `oxphp_portable_deserialize` recurses to read them on recv. A depth guard
/// in this walker alone would not prevent the overflow; the bound belongs in
/// the portbuf codec itself (C side), as a `Shared\*`-wide hardening shared by
/// `sv_read`/`sv_write`/the C serializer — out of scope for this function.
pub fn scan_shared_refs(
    buf: &[u8],
) -> Result<SmallVec<[SharedRef; 1]>, crate::plugins::ox_shared::error::SharedError> {
    let mut out: SmallVec<[SharedRef; 1]> = SmallVec::new();
    let mut pos = 0usize;
    scan_one(buf, &mut pos, &mut out)?;
    Ok(out)
}

fn scan_one(
    buf: &[u8],
    pos: &mut usize,
    out: &mut SmallVec<[SharedRef; 1]>,
) -> Result<(), crate::plugins::ox_shared::error::SharedError> {
    use crate::plugins::ox_shared::error::SharedError;
    fn rd_u8(b: &[u8], p: &mut usize) -> Result<u8, SharedError> {
        if *p >= b.len() {
            return Err(SharedError::Generic);
        }
        let v = b[*p];
        *p += 1;
        Ok(v)
    }
    fn rd_u32(b: &[u8], p: &mut usize) -> Result<u32, SharedError> {
        if *p + 4 > b.len() {
            return Err(SharedError::Generic);
        }
        let v = u32::from_le_bytes(b[*p..*p + 4].try_into().unwrap());
        *p += 4;
        Ok(v)
    }
    fn rd_u64(b: &[u8], p: &mut usize) -> Result<u64, SharedError> {
        if *p + 8 > b.len() {
            return Err(SharedError::Generic);
        }
        let v = u64::from_le_bytes(b[*p..*p + 8].try_into().unwrap());
        *p += 8;
        Ok(v)
    }
    fn skip(b: &[u8], p: &mut usize, n: usize) -> Result<(), SharedError> {
        if *p + n > b.len() {
            return Err(SharedError::Generic);
        }
        *p += n;
        Ok(())
    }

    let tag = rd_u8(buf, pos)?;
    match tag {
        0..=2 => {}                  // null / true / false — no body
        3..=4 => skip(buf, pos, 8)?, // long / double
        5 => {
            let n = rd_u32(buf, pos)? as usize; // string / bytes
            skip(buf, pos, n)?;
        }
        6 => {
            let count = rd_u32(buf, pos)? as usize;
            for _ in 0..count {
                let key_type = rd_u8(buf, pos)?;
                if key_type == 1 {
                    let klen = rd_u32(buf, pos)? as usize;
                    skip(buf, pos, klen)?;
                } else {
                    skip(buf, pos, 8)?; // u64 index key
                }
                scan_one(buf, pos, out)?;
            }
        }
        7 => {
            let tt = rd_u8(buf, pos)?;
            let id = rd_u64(buf, pos)?;
            let type_tag = SharedType::from_tag(tt).ok_or(SharedError::Type)?;
            out.push(SharedRef { id, type_tag });
        }
        _ => return Err(SharedError::Type),
    }
    Ok(())
}

/// Walk a [`SharedValueRaw`] tree and resolve every `Shared(SharedRef)`
/// node into `SharedValue::Shared(SharedRefOwned)` by looking up the
/// entry in `reg`. Returns [`SharedError::StaleHandle`] if any nested
/// reference points to an entry that has already dropped (the receiver
/// lost the cross-thread race; this is a clean error, not a bug).
///
/// Scalars and arrays are converted recursively. Arrays are unwrapped
/// from `Arc<SharedArrayRaw>` and rebuilt as `Arc<SharedArray>` — the
/// raw and owned variants do not share storage.
pub fn raw_to_owned(
    raw: SharedValueRaw,
    reg: &SharedRegistry,
) -> Result<SharedValue, crate::plugins::ox_shared::error::SharedError> {
    Ok(match raw {
        SharedValueRaw::Null => SharedValue::Null,
        SharedValueRaw::Bool(b) => SharedValue::Bool(b),
        SharedValueRaw::Long(v) => SharedValue::Long(v),
        SharedValueRaw::Double(v) => SharedValue::Double(v),
        SharedValueRaw::String(s) => SharedValue::String(s),
        SharedValueRaw::Bytes(b) => SharedValue::Bytes(b),
        SharedValueRaw::Array(a) => {
            // SharedArrayRaw is owned via Arc; try to unwrap, but if
            // shared, clone the contents.
            let arr_raw = Arc::try_unwrap(a).unwrap_or_else(|a| (*a).clone());
            let mut arr = SharedArray::default();
            arr.int_keyed.reserve(arr_raw.int_keyed.len());
            for v in arr_raw.int_keyed {
                arr.int_keyed.push(raw_to_owned(v, reg)?);
            }
            arr.str_keyed.reserve(arr_raw.str_keyed.len());
            for (k, v) in arr_raw.str_keyed {
                arr.str_keyed.push((k, raw_to_owned(v, reg)?));
            }
            SharedValue::Array(Arc::new(arr))
        }
        SharedValueRaw::Shared(r) => {
            let arc = reg.lookup(r.id)?;
            SharedValue::Shared(SharedRefOwned::from_arc(arc))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_bytes_scalars() {
        assert_eq!(SharedValue::Null.mem_bytes(), 1);
        assert_eq!(SharedValue::Bool(true).mem_bytes(), 1);
        assert_eq!(SharedValue::Long(42).mem_bytes(), 8);
        assert_eq!(SharedValue::Double(2.5).mem_bytes(), 8);
    }

    #[test]
    fn mem_bytes_string() {
        let v = SharedValue::String(Arc::from("hello"));
        assert_eq!(v.mem_bytes(), 5 + 16);
    }

    #[test]
    fn mem_bytes_bytes() {
        let v = SharedValue::Bytes(Arc::from([1u8, 2, 3].as_slice()));
        assert_eq!(v.mem_bytes(), 3 + 16);
    }

    // ── portbuf round-trip tests ──────────────────────────────────────────
    //
    // These tests cover the scalar + array path. None of them encode a
    // `Shared` variant — that path is exercised in `map.rs` /
    // `mutex.rs` integration tests which build a real `SharedRegistry`
    // and resolve via `raw_to_owned`. Here we compare the
    // `SharedValueRaw` decode output against an expected raw value
    // structurally.

    fn sv_to_raw_scalar(sv: &SharedValue) -> SharedValueRaw {
        match sv {
            SharedValue::Null => SharedValueRaw::Null,
            SharedValue::Bool(b) => SharedValueRaw::Bool(*b),
            SharedValue::Long(v) => SharedValueRaw::Long(*v),
            SharedValue::Double(v) => SharedValueRaw::Double(*v),
            SharedValue::String(s) => SharedValueRaw::String(Arc::clone(s)),
            SharedValue::Bytes(b) => SharedValueRaw::Bytes(Arc::clone(b)),
            _ => panic!("sv_to_raw_scalar: not a scalar"),
        }
    }

    #[test]
    fn portbuf_roundtrip_scalars() {
        for sv in [
            SharedValue::Null,
            SharedValue::Bool(true),
            SharedValue::Bool(false),
            SharedValue::Long(-42),
            SharedValue::Long(i64::MAX),
            SharedValue::Double(2.5),
            SharedValue::String(Arc::from("hello world")),
        ] {
            let bytes = sv_to_portbuf(&sv);
            let decoded = portbuf_to_sv(&bytes).unwrap();
            let expected = sv_to_raw_scalar(&sv);
            assert_eq!(format!("{expected:?}"), format!("{decoded:?}"));
        }
    }

    #[test]
    fn portbuf_roundtrip_array() {
        let mut arr = SharedArray::default();
        arr.int_keyed.push(SharedValue::Long(10));
        arr.int_keyed.push(SharedValue::Long(20));
        arr.str_keyed
            .push((Arc::from("key"), SharedValue::String(Arc::from("val"))));
        let sv = SharedValue::Array(Arc::new(arr));
        let bytes = sv_to_portbuf(&sv);
        let decoded = portbuf_to_sv(&bytes).unwrap();
        // Expected raw — structurally identical with scalar leaves.
        let mut arr_raw = SharedArrayRaw::default();
        arr_raw.int_keyed.push(SharedValueRaw::Long(10));
        arr_raw.int_keyed.push(SharedValueRaw::Long(20));
        arr_raw
            .str_keyed
            .push((Arc::from("key"), SharedValueRaw::String(Arc::from("val"))));
        let expected = SharedValueRaw::Array(Arc::new(arr_raw));
        assert_eq!(format!("{expected:?}"), format!("{decoded:?}"));
    }

    #[test]
    fn portbuf_empty_buf_errors() {
        assert!(portbuf_to_sv(&[]).is_err());
    }

    #[test]
    fn scan_finds_nothing_in_scalars() {
        for sv in [
            SharedValue::Null,
            SharedValue::Bool(true),
            SharedValue::Long(-42),
            SharedValue::Double(2.5),
            SharedValue::String(Arc::from("hello")),
        ] {
            let bytes = sv_to_portbuf(&sv);
            assert!(scan_shared_refs(&bytes).unwrap().is_empty());
        }
    }

    #[test]
    fn scan_finds_bare_shared_ref() {
        // tag-7: type_tag byte + u64 LE id.
        let mut bytes = vec![7u8, SharedType::Counter as u8];
        bytes.extend_from_slice(&99u64.to_le_bytes());
        let refs = scan_shared_refs(&bytes).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, 99);
        assert_eq!(refs[0].type_tag, SharedType::Counter);
    }

    #[test]
    fn scan_recurses_into_array_with_nested_shared() {
        // Array [ 0 => Long(1), "k" => Shared(Map#8) ]
        let mut b = vec![6u8]; // tag array
        b.extend_from_slice(&2u32.to_le_bytes()); // count = 2
                                                  // int-keyed Long(1)
        b.push(0);
        b.extend_from_slice(&0u64.to_le_bytes());
        b.push(3);
        b.extend_from_slice(&1i64.to_le_bytes());
        // str-keyed "k" => Shared(Map#8)
        b.push(1);
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(b'k');
        b.push(7);
        b.push(SharedType::Map as u8);
        b.extend_from_slice(&8u64.to_le_bytes());
        let refs = scan_shared_refs(&b).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].id, 8);
        assert_eq!(refs[0].type_tag, SharedType::Map);
    }

    #[test]
    fn scan_errors_on_truncated_buffer() {
        assert!(scan_shared_refs(&[]).is_err());
        assert!(scan_shared_refs(&[7u8]).is_err()); // tag-7 missing body
        assert!(scan_shared_refs(&[5u8, 10, 0, 0, 0]).is_err()); // string len=10, no data
    }
}
