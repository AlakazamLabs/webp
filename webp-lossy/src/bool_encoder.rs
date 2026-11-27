//! The boolean entropy encoder of RFC 6386 Section 7.
//!
//! Written from the RFC's description of the coder (Sections 7.1 to 7.3):
//! the interval `[bottom, bottom + range)` is split in proportion to the
//! probability of a zero, `range` is kept in 128..=255 by doubling, and
//! the bits shifted out of `bottom` become output bytes, with a carry
//! propagated back into bytes already written.

/// Writes bools at given probabilities into one partition.
pub(crate) struct BoolEncoder {
    out: Vec<u8>,
    range: u32,
    bottom: u32,
    /// Shifts left before the top byte of `bottom` is complete.
    bit_count: i32,
}

impl BoolEncoder {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_capacity(0)
    }

    pub(crate) fn with_capacity(bytes: usize) -> Self {
        BoolEncoder {
            out: Vec::with_capacity(bytes),
            range: 255,
            bottom: 0,
            bit_count: 24,
        }
    }

    /// Adds one to the bytes already written (a carry out of `bottom`).
    fn carry(&mut self) {
        for b in self.out.iter_mut().rev() {
            if *b == 255 {
                *b = 0;
            } else {
                *b += 1;
                return;
            }
        }
    }

    /// Writes `bit`, whose probability of being zero is `prob / 256`.
    #[inline]
    pub(crate) fn write(&mut self, prob: u8, bit: bool) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if bit {
            self.bottom = self.bottom.wrapping_add(split);
            self.range -= split;
        } else {
            self.range = split;
        }
        // Renormalise: one shift per leading zero of the 8-bit range.
        let shift = self.range.leading_zeros().saturating_sub(24);
        for _ in 0..shift {
            self.range <<= 1;
            if self.bottom & (1 << 31) != 0 {
                self.carry();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.out.push((self.bottom >> 24) as u8);
                self.bottom &= (1 << 24) - 1;
                self.bit_count = 8;
            }
        }
    }

    /// Writes a bit at probability 1/2.
    #[inline]
    pub(crate) fn write_bit(&mut self, bit: bool) {
        self.write(128, bit);
    }

    /// Writes the low `bits` bits of `value`, most significant first, each at
    /// probability 1/2 (the RFC's `L(n)`).
    pub(crate) fn write_literal(&mut self, value: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.write_bit((value >> i) & 1 == 1);
        }
    }

    /// Writes a magnitude of `bits` bits followed by a sign bit (1 for
    /// negative), the form of the header's signed fields.
    pub(crate) fn write_signed(&mut self, value: i32, bits: u32) {
        self.write_literal(value.unsigned_abs(), bits);
        self.write_bit(value < 0);
    }

    /// Writes `value` as a leaf of `tree` (RFC 6386 Section 8.1's layout:
    /// pairs of branches, a positive entry is the index of the next pair, a
    /// non-positive entry `-v` is the leaf `v`), with one probability per
    /// pair.
    pub(crate) fn write_tree(&mut self, tree: &[i8], probs: &[u8], value: u8) {
        let mut path = [(0usize, false); 16];
        let n = tree_path(tree, value, &mut path);
        for &(node, bit) in &path[..n] {
            self.write(probs[node >> 1], bit);
        }
    }

    /// Ends the partition and returns its bytes.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        // Shift out the pending bits of `bottom`, padded with zeros, until
        // four more bytes are written (as the RFC's flush does), so that a
        // decoder reading ahead always finds bytes within the partition.
        let mut pushed = 0;
        while pushed < 4 {
            if self.bottom & (1 << 31) != 0 {
                self.carry();
            }
            self.bottom <<= 1;
            self.bit_count -= 1;
            if self.bit_count == 0 {
                self.out.push((self.bottom >> 24) as u8);
                self.bottom &= (1 << 24) - 1;
                self.bit_count = 8;
                pushed += 1;
            }
        }
        self.out
    }

    /// Bytes written so far (the partition grows by about this much).
    pub(crate) fn len(&self) -> usize {
        self.out.len()
    }
}

/// The branch decisions from the root of `tree` to the leaf `value`:
/// `(node index, bit)` pairs written into `path`; returns how many.
pub(crate) fn tree_path(tree: &[i8], value: u8, path: &mut [(usize, bool); 16]) -> usize {
    fn walk(tree: &[i8], node: usize, value: u8, depth: usize, path: &mut [(usize, bool); 16]) -> Option<usize> {
        for bit in [false, true] {
            path[depth] = (node, bit);
            let next = tree[node + bit as usize];
            if next <= 0 {
                if (-next) as u8 == value {
                    return Some(depth + 1);
                }
            } else if let Some(n) = walk(tree, next as usize, value, depth + 1, path) {
                return Some(n);
            }
        }
        None
    }
    walk(tree, 0, value, 0, path).expect("value is a leaf of the tree")
}

#[cfg(test)]
pub(crate) mod test_decoder {
    //! A boolean decoder for the tests, written from RFC 6386 Section 7.3's
    //! description of the decoder (the exact parallel of the encoder).

    pub(crate) struct BoolDecoder<'a> {
        data: &'a [u8],
        pos: usize,
        value: u32,
        range: u32,
        bit_count: u32,
    }

    impl<'a> BoolDecoder<'a> {
        pub(crate) fn new(data: &'a [u8]) -> Self {
            let mut d = BoolDecoder {
                data,
                pos: 0,
                value: 0,
                range: 255,
                bit_count: 0,
            };
            for _ in 0..2 {
                d.value = (d.value << 8) | d.next_byte();
            }
            d
        }

        fn next_byte(&mut self) -> u32 {
            let b = self.data.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            b as u32
        }

        pub(crate) fn read(&mut self, prob: u8) -> bool {
            let split = 1 + (((self.range - 1) * prob as u32) >> 8);
            let big_split = split << 8;
            let bit = if self.value >= big_split {
                self.range -= split;
                self.value -= big_split;
                true
            } else {
                self.range = split;
                false
            };
            while self.range < 128 {
                self.value <<= 1;
                self.range <<= 1;
                self.bit_count += 1;
                if self.bit_count == 8 {
                    self.bit_count = 0;
                    self.value |= self.next_byte();
                }
            }
            bit
        }

        pub(crate) fn read_literal(&mut self, bits: u32) -> u32 {
            let mut v = 0;
            for _ in 0..bits {
                v = (v << 1) | self.read(128) as u32;
            }
            v
        }

        pub(crate) fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
            let mut i = 0usize;
            loop {
                let next = tree[i + self.read(probs[i >> 1]) as usize];
                if next <= 0 {
                    return (-next) as u8;
                }
                i = next as usize;
            }
        }

        /// Bytes consumed so far, counting the two read ahead.
        pub(crate) fn position(&self) -> usize {
            self.pos
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_decoder::BoolDecoder;
    use super::*;

    fn rnd(seed: &mut u32) -> u32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed
    }

    #[test]
    fn random_bools_round_trip() {
        let mut seed = 12345u32;
        for len in [0usize, 1, 2, 7, 100, 10_000] {
            let items: Vec<(u8, bool)> = (0..len)
                .map(|_| {
                    let p = (rnd(&mut seed) % 256) as u8;
                    let p = p.max(1);
                    // Mostly follow the probability, so long runs of carries happen too.
                    let bit = (rnd(&mut seed) % 256) as u8 >= p;
                    (p, bit)
                })
                .collect();
            let mut e = BoolEncoder::new();
            for &(p, b) in &items {
                e.write(p, b);
            }
            let data = e.finish();
            let mut d = BoolDecoder::new(&data);
            for (i, &(p, b)) in items.iter().enumerate() {
                assert_eq!(d.read(p), b, "bool {i} of {len}");
            }
            assert!(d.position() <= data.len() + 2);
            assert!(data.len() >= 4, "a partition is at least the four flushed bytes");
        }
    }

    #[test]
    fn extreme_probabilities_round_trip() {
        // Probability 255 with ones and 1 with zeros: long carry chains.
        let mut e = BoolEncoder::new();
        for i in 0..5000 {
            e.write(255, i % 97 != 0);
            e.write(1, i % 89 == 0);
        }
        let data = e.finish();
        let mut d = BoolDecoder::new(&data);
        for i in 0..5000 {
            assert_eq!(d.read(255), i % 97 != 0);
            assert_eq!(d.read(1), i % 89 == 0);
        }
    }

    #[test]
    fn literals_and_trees_round_trip() {
        const TREE: [i8; 8] = [-0, 2, -1, 4, -2, 6, -3, -4];
        let probs = [200u8, 30, 128, 77];
        let mut e = BoolEncoder::new();
        for v in 0..5u8 {
            e.write_tree(&TREE, &probs, v);
            e.write_literal(v as u32 * 25, 7);
        }
        let data = e.finish();
        let mut d = BoolDecoder::new(&data);
        for v in 0..5u8 {
            assert_eq!(d.read_tree(&TREE, &probs), v);
            assert_eq!(d.read_literal(7), v as u32 * 25);
        }
    }
}
