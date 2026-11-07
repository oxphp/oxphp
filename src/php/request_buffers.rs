//! Buffers a worker thread keeps from one request to the next.
//!
//! A request's `$_SERVER`, headers, cookies and query pairs are dozens of short
//! strings that all die together. Owned one string at a time they are most of
//! the allocations a request makes; kept in buffers that outlive the request
//! they cost none once the buffers have reached the size the traffic needs.
//!
//! Kept out of `sapi.rs` so it builds and gets tested without linking PHP.

use std::os::raw::c_char;

use bytes::Bytes;

/// Bytes a buffer keeps between requests, so one request with unusually long
/// headers does not pin that memory on the worker thread for good.
pub(crate) const RETAINED_BYTES: usize = 32 * 1024;

/// Pairs a list keeps room for between requests, for the same reason.
pub(crate) const RETAINED_PAIRS: usize = 256;

/// Output buffer kept between requests. Past it a response hands its buffer
/// over instead of being copied out of it, which is also where a copy would
/// start to cost more than the allocation it saves.
const RETAINED_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
struct PackedPair {
    start: usize,
    name_len: usize,
    value_len: usize,
}

impl PackedPair {
    fn in_buffer(self, bytes: &[u8]) -> (&[u8], &[u8]) {
        let value_start = self.start + self.name_len + 1;
        (
            &bytes[self.start..self.start + self.name_len],
            &bytes[value_start..value_start + self.value_len],
        )
    }
}

/// Name/value byte strings laid end to end in one buffer.
///
/// Every name and value is followed by a NUL in the buffer, so where it holds
/// no NUL of its own the pointer of either is a C string as it stands. Pairs
/// keep their order and a name may repeat.
#[derive(Default)]
pub struct PackedPairs {
    bytes: Vec<u8>,
    pairs: Vec<PackedPair>,
}

impl PackedPairs {
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Forget the pairs and keep the buffers.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.pairs.clear();
    }

    /// [`clear`](Self::clear), giving back what an oversized request left.
    pub fn reset(&mut self) {
        self.clear();
        self.bytes.shrink_to(RETAINED_BYTES);
        self.pairs.shrink_to(RETAINED_PAIRS);
    }

    pub fn push(&mut self, name: &[u8], value: &[u8]) {
        self.push_with(
            |out| out.extend_from_slice(name),
            |out| out.extend_from_slice(value),
        );
    }

    /// Append a pair whose name and value the closures write into the buffer,
    /// for one that would otherwise need a buffer of its own to be built in.
    pub fn push_with(&mut self, name: impl FnOnce(&mut Vec<u8>), value: impl FnOnce(&mut Vec<u8>)) {
        let start = self.bytes.len();
        name(&mut self.bytes);
        let name_len = self.bytes.len() - start;
        self.bytes.push(0);
        let value_start = self.bytes.len();
        value(&mut self.bytes);
        let value_len = self.bytes.len() - value_start;
        self.bytes.push(0);
        self.pairs.push(PackedPair {
            start,
            name_len,
            value_len,
        });
    }

    /// [`push_with`](Self::push_with) for a pair that goes to C as two C
    /// strings: one holding a NUL of its own could not, and is left out.
    pub fn push_c_with(
        &mut self,
        name: impl FnOnce(&mut Vec<u8>),
        value: impl FnOnce(&mut Vec<u8>),
    ) {
        self.push_with(name, value);
        let Some(&last) = self.pairs.last() else {
            return;
        };
        let (name, value) = last.in_buffer(&self.bytes);
        if name.contains(&0) || value.contains(&0) {
            self.pairs.pop();
            self.bytes.truncate(last.start);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.pairs.iter().map(|&pair| pair.in_buffer(&self.bytes))
    }

    /// The buffers by address, for a walk no borrow can stay alive across.
    pub fn as_raw(&self) -> RawPairs {
        RawPairs {
            bytes: self.bytes.as_slice(),
            pairs: self.pairs.as_slice(),
        }
    }

    /// Each pair's name, with its value as a slice of one shared copy of the
    /// buffer, which is what lets a response's header values outlive these
    /// buffers for two allocations however many values there are.
    pub fn iter_shared_values(&self) -> impl Iterator<Item = (&[u8], Bytes)> {
        let shared = Bytes::copy_from_slice(&self.bytes);
        self.pairs.iter().map(move |&pair| {
            let value_start = pair.start + pair.name_len + 1;
            (
                &self.bytes[pair.start..pair.start + pair.name_len],
                shared.slice(value_start..value_start + pair.value_len),
            )
        })
    }

    /// Keep the pairs `keep` says to, closing the gaps the others leave so a
    /// script that keeps replacing one header does not grow the buffer. Runs on
    /// every `header()` call that replaces, usually removing nothing, so a pair
    /// with no gap before it is not moved.
    pub fn retain(&mut self, mut keep: impl FnMut(&[u8], &[u8]) -> bool) {
        let mut kept_end = 0;
        let bytes = &mut self.bytes;
        self.pairs.retain_mut(|pair| {
            let (name, value) = pair.in_buffer(bytes);
            if !keep(name, value) {
                return false;
            }
            let len = pair.name_len + pair.value_len + 2;
            if pair.start != kept_end {
                bytes.copy_within(pair.start..pair.start + len, kept_end);
                pair.start = kept_end;
            }
            kept_end += len;
            true
        });
        bytes.truncate(kept_end);
    }

    #[cfg(all(test, feature = "php"))]
    pub(crate) fn capacities(&self) -> (usize, usize) {
        (self.bytes.capacity(), self.pairs.capacity())
    }
}

/// A [`PackedPairs`]'s buffers by address. They stay where they are when the
/// value that owns them moves, which a reference to that value would not survive.
#[derive(Clone, Copy)]
pub struct RawPairs {
    bytes: *const [u8],
    pairs: *const [PackedPair],
}

impl RawPairs {
    /// # Safety
    ///
    /// For as long as the iterator and what it yields live, the [`PackedPairs`]
    /// this came from must not be changed or dropped. It may be moved.
    pub unsafe fn iter<'a>(self) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
        // SAFETY: the caller keeps both buffers alive and unchanged for `'a`.
        let (bytes, pairs) = unsafe { (&*self.bytes, &*self.pairs) };
        pairs.iter().map(move |&pair| pair.in_buffer(bytes))
    }
}

/// Empty a string the way [`PackedPairs::reset`] empties its buffers.
pub fn reset_string(s: &mut String) {
    s.clear();
    s.shrink_to(RETAINED_BYTES);
}

/// A C string whose buffer is kept from one request to the next.
#[derive(Default)]
pub struct ReusedCStr(Vec<u8>);

impl ReusedCStr {
    /// Hold `s`, or nothing where it has a NUL a C string could not carry.
    pub fn set(&mut self, s: &[u8]) {
        self.reset();
        if !s.contains(&0) {
            self.0.extend_from_slice(s);
            self.0.push(0);
        }
    }

    /// Hold nothing, giving back what an oversized request left.
    pub fn reset(&mut self) {
        self.0.clear();
        self.0.shrink_to(RETAINED_BYTES);
    }

    /// The C string, or null when there is none. Valid until the next `set`.
    pub fn as_ptr(&self) -> *const c_char {
        if self.0.is_empty() {
            std::ptr::null()
        } else {
            self.0.as_ptr() as *const c_char
        }
    }

    /// The string without its terminator, when there is one.
    pub fn to_bytes(&self) -> Option<&[u8]> {
        self.0.split_last().map(|(_, bytes)| bytes)
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.0.capacity()
    }
}

/// Empty a response's output buffer into a body that can leave the thread.
///
/// Moving the `Vec` out looks free, but the next request then grows a buffer
/// from nothing, one reallocation per doubling, and `Bytes` boxes a header for
/// a `Vec` with spare capacity on top. Copying out of a buffer that stays costs
/// one allocation of the exact size.
pub fn take_output(output: &mut Vec<u8>) -> Bytes {
    if output.len() > RETAINED_OUTPUT_BYTES {
        return Bytes::from(std::mem::replace(
            output,
            Vec::with_capacity(RETAINED_OUTPUT_BYTES),
        ));
    }
    let body = Bytes::copy_from_slice(output);
    output.clear();
    output.shrink_to(RETAINED_OUTPUT_BYTES);
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn collect(pairs: &PackedPairs) -> Vec<(Vec<u8>, Vec<u8>)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_vec(), value.to_vec()))
            .collect()
    }

    fn pair(name: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
        (name.as_bytes().to_vec(), value.as_bytes().to_vec())
    }

    #[test]
    fn pairs_come_back_in_order_with_repeats_and_empties() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"a", b"1");
        pairs.push(b"", b"");
        pairs.push(b"a", b"2");
        assert_eq!(pairs.len(), 3);
        assert_eq!(
            collect(&pairs),
            [pair("a", "1"), pair("", ""), pair("a", "2")]
        );
    }

    #[test]
    fn names_and_values_are_c_strings_in_place() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"KEY", b"value");
        pairs.push(b"OTHER", b"");
        for (name, value) in pairs.iter() {
            // SAFETY: both point into `pairs`, each followed by its NUL.
            unsafe {
                assert_eq!(
                    CStr::from_ptr(name.as_ptr() as *const c_char).to_bytes(),
                    name
                );
                assert_eq!(
                    CStr::from_ptr(value.as_ptr() as *const c_char).to_bytes(),
                    value
                );
            }
        }
    }

    #[test]
    fn a_plain_push_keeps_a_nul_a_decoded_value_may_hold() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"k", b"\x00\xff");
        assert_eq!(collect(&pairs), [(b"k".to_vec(), b"\x00\xff".to_vec())]);
    }

    #[test]
    fn a_pair_bound_for_c_is_left_out_whole_when_it_holds_a_nul() {
        let mut pairs = PackedPairs::default();
        let bytes = |s: &'static [u8]| move |out: &mut Vec<u8>| out.extend_from_slice(s);
        pairs.push_c_with(bytes(b"A"), bytes(b"1"));
        pairs.push_c_with(bytes(b"B\0"), bytes(b"2"));
        pairs.push_c_with(bytes(b"C"), bytes(b"3\0"));
        pairs.push_c_with(bytes(b"D"), bytes(b"4"));
        assert_eq!(collect(&pairs), [pair("A", "1"), pair("D", "4")]);
    }

    #[test]
    fn shared_values_are_the_values_of_each_pair() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"A", b"1");
        pairs.push(b"Empty", b"");
        pairs.push(b"B", b"two");

        let listed: Vec<(Vec<u8>, Bytes)> = pairs
            .iter_shared_values()
            .map(|(name, value)| (name.to_vec(), value))
            .collect();

        assert_eq!(
            listed,
            [
                (b"A".to_vec(), Bytes::from_static(b"1")),
                (b"Empty".to_vec(), Bytes::new()),
                (b"B".to_vec(), Bytes::from_static(b"two")),
            ]
        );
    }

    #[test]
    fn retain_removes_pairs_and_closes_the_gaps() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"drop", b"first");
        pairs.push(b"keep", b"a");
        pairs.push(b"drop", b"middle");
        pairs.push(b"keep", b"");
        pairs.push(b"drop", b"last");
        let before = pairs.bytes.len();
        pairs.retain(|name, _| name != b"drop");
        assert_eq!(collect(&pairs), [pair("keep", "a"), pair("keep", "")]);
        assert!(pairs.bytes.len() < before);
        assert_eq!(pairs.bytes, b"keep\0a\0keep\0\0");
        // The list still takes new pairs where the kept ones end.
        pairs.push(b"new", b"x");
        assert_eq!(
            collect(&pairs),
            [pair("keep", "a"), pair("keep", ""), pair("new", "x")]
        );
    }

    #[test]
    fn replacing_one_pair_over_and_over_does_not_grow_the_buffer() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"x-replaced", b"0");
        let settled = pairs.bytes.len();
        for _ in 0..1000 {
            pairs.retain(|name, _| name != b"x-replaced");
            pairs.push(b"x-replaced", b"0");
        }
        assert_eq!(pairs.bytes.len(), settled);
    }

    #[test]
    fn clear_keeps_the_buffers_and_reset_gives_back_an_oversized_one() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"k", &[b'v'; 1024]);
        let capacity = pairs.bytes.capacity();
        pairs.clear();
        assert!(pairs.is_empty());
        assert_eq!(pairs.bytes.capacity(), capacity);

        pairs.push(b"k", &vec![b'v'; RETAINED_BYTES * 4]);
        pairs.reset();
        assert!(pairs.is_empty());
        assert!(pairs.bytes.capacity() <= RETAINED_BYTES);
    }

    #[test]
    fn a_reused_c_string_holds_what_it_was_last_given() {
        let mut s = ReusedCStr::default();
        assert!(s.as_ptr().is_null());
        assert_eq!(s.to_bytes(), None);

        s.set(b"longer value");
        s.set(b"short");
        assert_eq!(s.to_bytes(), Some(&b"short"[..]));
        // SAFETY: `s` holds a NUL-terminated string.
        assert_eq!(unsafe { CStr::from_ptr(s.as_ptr()) }.to_bytes(), b"short");

        s.set(b"");
        assert_eq!(s.to_bytes(), Some(&b""[..]));
        assert!(!s.as_ptr().is_null());
    }

    #[test]
    fn a_walk_by_address_survives_the_pairs_moving() {
        let mut pairs = PackedPairs::default();
        pairs.push(b"a", b"1");
        pairs.push(b"b", b"2");
        let raw = pairs.as_raw();
        let moved = Box::new(pairs);
        // SAFETY: `moved` is alive and unchanged for the whole walk.
        let walked: Vec<_> = unsafe { raw.iter() }
            .map(|(name, value)| (name.to_vec(), value.to_vec()))
            .collect();
        assert_eq!(walked, [pair("a", "1"), pair("b", "2")]);
        drop(moved);
    }

    #[test]
    fn a_reused_c_string_gives_back_an_oversized_buffer_when_set_again() {
        let mut s = ReusedCStr::default();
        s.set(&vec![b'x'; RETAINED_BYTES * 4]);
        s.set(b"short");
        assert_eq!(s.to_bytes(), Some(&b"short"[..]));
        assert!(s.capacity() <= RETAINED_BYTES);
    }

    #[test]
    fn a_reset_string_keeps_a_normal_buffer_and_gives_back_an_oversized_one() {
        let mut s = String::with_capacity(RETAINED_BYTES);
        s.push('x');
        let kept = s.as_ptr();
        reset_string(&mut s);
        assert!(s.is_empty());
        assert_eq!(s.as_ptr(), kept);

        let mut s = String::with_capacity(RETAINED_BYTES * 4);
        reset_string(&mut s);
        assert!(s.capacity() <= RETAINED_BYTES);
    }

    #[test]
    fn a_reused_c_string_refuses_an_embedded_nul() {
        let mut s = ReusedCStr::default();
        s.set(b"kept");
        s.set(b"a\0b");
        assert!(s.as_ptr().is_null());
    }

    #[test]
    fn take_output_keeps_a_small_buffer_for_the_next_request() {
        let mut output = Vec::with_capacity(4096);
        output.extend_from_slice(b"body");
        let kept = output.as_ptr();
        assert_eq!(&take_output(&mut output)[..], b"body");
        assert!(output.is_empty());
        assert_eq!(output.as_ptr(), kept);
        assert_eq!(output.capacity(), 4096);
    }

    #[test]
    fn take_output_hands_a_large_buffer_over_without_copying_it() {
        let mut output = vec![b'x'; RETAINED_OUTPUT_BYTES + 1];
        let handed_over = output.as_ptr();
        let body = take_output(&mut output);
        assert_eq!(body.as_ptr(), handed_over);
        assert_eq!(body.len(), RETAINED_OUTPUT_BYTES + 1);
        assert!(output.is_empty());
        assert_eq!(output.capacity(), RETAINED_OUTPUT_BYTES);
    }

    #[test]
    fn take_output_gives_back_a_buffer_a_spool_left_empty() {
        let mut output = Vec::with_capacity(RETAINED_OUTPUT_BYTES * 8);
        assert!(take_output(&mut output).is_empty());
        assert!(output.capacity() <= RETAINED_OUTPUT_BYTES);
    }
}
