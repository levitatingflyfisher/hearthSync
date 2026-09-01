//! The nesting guard in front of every dCBOR decode (ADR 0003).
//!
//! dcbor 0.25 decodes recursively, one stack frame per nesting level, with no limit:
//! 65,000 bytes of `0x81` (arrays inside arrays) abort the process on a 2 MiB stack,
//! and a WASM stack is smaller still. An op, an envelope, a sync message, a snapshot
//! and a stored record can all come from someone else, so the kernel decodes none of
//! them without [`nesting_ok`] first. The relay calls the same function (it
//! re-exports it from `wire`), so there is one implementation.

use dcbor::CBOR;

/// The deepest nesting the kernel accepts in anything it decodes. The deepest thing
/// the kernel itself encodes is 7 deep (measured by instrumenting the decoder across
/// the whole test suite; ADR 0003), so 16 leaves more than twice the room, and 16
/// decoder frames are harmless on any stack. The relay's requests use their own,
/// tighter limit (`wire::MAX_DEPTH`).
pub const MAX_DEPTH: usize = 16;

/// True if `data` is one well-formed CBOR item nested at most `max` deep, without
/// recursing. Depth counts levels of items: a lone scalar is 1 deep, `[0]` is 2. It checks structure only (heads and lengths); dCBOR's own decoder does the
/// rest once this has ruled out input deep enough to exhaust the stack.
pub fn nesting_ok(data: &[u8], max: usize) -> bool {
    // Items still to read at each open level; the root level holds one.
    let mut stack: Vec<u64> = vec![1];
    let mut pos = 0usize;
    while let Some(top) = stack.last_mut() {
        if *top == 0 {
            stack.pop();
            continue;
        }
        *top -= 1;
        let Some(&h) = data.get(pos) else { return false };
        pos += 1;
        let (major, ai) = (h >> 5, h & 31);
        let arg = match ai {
            0..=23 => ai as u64,
            24..=27 => {
                let n = 1usize << (ai - 24);
                let Some(b) = data.get(pos..pos + n) else { return false };
                pos += n;
                b.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
            }
            _ => return false, // reserved or indefinite length: never dCBOR
        };
        match major {
            0 | 1 | 7 => {}
            2 | 3 => {
                let Some(end) = usize::try_from(arg).ok().and_then(|n| pos.checked_add(n)) else { return false };
                if end > data.len() {
                    return false;
                }
                pos = end;
            }
            4..=6 => {
                let items = match major {
                    4 => arg,
                    5 => match arg.checked_mul(2) {
                        Some(n) => n,
                        None => return false,
                    },
                    _ => 1,
                };
                // Each item needs at least one byte.
                if items > (data.len() - pos) as u64 {
                    return false;
                }
                if items > 0 {
                    if stack.len() >= max {
                        return false;
                    }
                    stack.push(items);
                }
            }
            _ => unreachable!(),
        }
    }
    pos == data.len()
}

/// Decode one dCBOR item, refusing anything nested deeper than [`MAX_DEPTH`] before
/// the recursive decoder sees it. The kernel's only way into `CBOR::try_from_data`.
pub(crate) fn decode(data: &[u8]) -> Option<CBOR> {
    if !nesting_ok(data, MAX_DEPTH) {
        return None;
    }
    CBOR::try_from_data(data).ok()
}
