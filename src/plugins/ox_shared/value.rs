//! SharedValue — the internal value type stored inside Shared\*
//! containers. Supports scalars plus Array + Shared nesting (used by
//! Map).

use std::sync::Arc;

use crate::plugins::ox_shared::registry::{SharedId, SharedRegistry, SharedType};

#[derive(Clone, Debug)]
pub enum SharedValue {
    Null,
    Bool(bool),
    Long(i64),
    Double(f64),
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    #[allow(dead_code)]
    Array(Arc<SharedArray>),
    #[allow(dead_code)]
    Shared(SharedRef),
}

#[derive(Clone, Debug, Default)]
pub struct SharedArray {
    pub int_keyed: Vec<SharedValue>,
    pub str_keyed: Vec<(Arc<str>, SharedValue)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SharedRef {
    pub id: SharedId,
    pub type_tag: SharedType,
}

/// Push every `SharedValue::Shared` reachable from `sv` into `out`.
/// Recurses into `SharedValue::Array`. Used by the Map cycle-check
/// to extract roots before calling the walker, and by
/// `MapInner::children` to expose outgoing edges to the walker.
pub fn collect_shared_refs(sv: &SharedValue, out: &mut Vec<SharedRef>) {
    match sv {
        SharedValue::Shared(r) => out.push(*r),
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

/// Walk every `SharedValue::Shared` in the value tree and call
/// `reg.retain(id)`. Containers call this before storing a value so the
/// nested Shareable stays alive for as long as the container holds it.
/// Arrays are recursed into; scalars are zero-cost.
///
/// Spec: 02-value-model.md §Nested `Shared\*` (SharedValue::Shared).
pub fn sv_retain_nested(sv: &SharedValue, reg: &SharedRegistry) {
    match sv {
        SharedValue::Shared(r) => {
            reg.retain(r.id);
        }
        SharedValue::Array(a) => {
            for v in &a.int_keyed {
                sv_retain_nested(v, reg);
            }
            for (_, v) in &a.str_keyed {
                sv_retain_nested(v, reg);
            }
        }
        _ => {}
    }
}

/// Symmetric to [`sv_retain_nested`]. Containers call this when they
/// drop a stored value and give up their hold on the nested Shareable.
pub fn sv_release_nested(sv: &SharedValue, reg: &SharedRegistry) {
    match sv {
        SharedValue::Shared(r) => {
            reg.release(r.id);
        }
        SharedValue::Array(a) => {
            for v in &a.int_keyed {
                sv_release_nested(v, reg);
            }
            for (_, v) in &a.str_keyed {
                sv_release_nested(v, reg);
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

/// Deserialise a `SharedValue` from the portbuf wire format.
/// Returns the first complete value in the buffer; extra bytes are ignored.
pub fn portbuf_to_sv(
    buf: &[u8],
) -> Result<SharedValue, crate::plugins::ox_shared::error::SharedError> {
    let mut pos = 0usize;
    sv_read(buf, &mut pos)
}

fn sv_read(
    buf: &[u8],
    pos: &mut usize,
) -> Result<SharedValue, crate::plugins::ox_shared::error::SharedError> {
    use crate::plugins::ox_shared::error::SharedError;
    use crate::plugins::ox_shared::registry::SharedType;

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
        0 => SharedValue::Null,
        1 => SharedValue::Bool(true),
        2 => SharedValue::Bool(false),
        3 => SharedValue::Long(read_i64_le(buf, pos)?),
        4 => SharedValue::Double(read_f64_le(buf, pos)?),
        5 => {
            let len = read_u32_le(buf, pos)? as usize;
            let bytes = read_bytes(buf, pos, len)?;
            match std::str::from_utf8(bytes) {
                Ok(s) => SharedValue::String(Arc::from(s)),
                Err(_) => SharedValue::Bytes(Arc::from(bytes)),
            }
        }
        6 => {
            let count = read_u32_le(buf, pos)? as usize;
            let mut arr = SharedArray::default();
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
            SharedValue::Array(Arc::new(arr))
        }
        7 => {
            let tt_byte = read_u8(buf, pos)?;
            let id = read_u64_le(buf, pos)?;
            let type_tag = SharedType::from_tag(tt_byte).ok_or(SharedError::Type)?;
            SharedValue::Shared(SharedRef { id, type_tag })
        }
        _ => return Err(SharedError::Type),
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
            assert_eq!(format!("{sv:?}"), format!("{decoded:?}"));
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
        assert_eq!(format!("{sv:?}"), format!("{decoded:?}"));
    }

    #[test]
    fn portbuf_empty_buf_errors() {
        assert!(portbuf_to_sv(&[]).is_err());
    }
}
