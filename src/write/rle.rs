//! PackBits compression, the run length encoding that PSD files use.
//!
//! See <https://en.wikipedia.org/wiki/PackBits>. The matching decoder lives in
//! [`crate::psd_channel`].

/// The largest number of bytes that a single PackBits packet can describe.
const MAX_PACKET_LEN: usize = 128;

/// PackBits compress a single scanline.
pub(crate) fn pack_bits(scanline: &[u8]) -> Vec<u8> {
    // Worst case is one header byte for every 128 literal bytes.
    let mut packed = Vec::with_capacity(scanline.len() + scanline.len() / MAX_PACKET_LEN + 1);

    let mut idx = 0;
    while idx < scanline.len() {
        let run = run_length(scanline, idx);

        if is_worth_encoding_as_run(scanline, idx, run) {
            // A repeat packet's header is `1 - run`, stored as a signed byte.
            packed.push((257 - run) as u8);
            packed.push(scanline[idx]);

            idx += run;
        } else {
            let literal_start = idx;

            while idx < scanline.len() && idx - literal_start < MAX_PACKET_LEN {
                let run = run_length(scanline, idx);
                if is_worth_encoding_as_run(scanline, idx, run) {
                    break;
                }

                idx += 1;
            }

            let literal = &scanline[literal_start..idx];

            // A literal packet's header is `len - 1`.
            packed.push((literal.len() - 1) as u8);
            packed.extend_from_slice(literal);
        }
    }

    packed
}

/// How many times in a row the byte at `idx` repeats, capped at the largest
/// run that a single packet can describe.
fn run_length(scanline: &[u8], idx: usize) -> usize {
    let byte = scanline[idx];

    let mut run = 1;
    while idx + run < scanline.len() && run < MAX_PACKET_LEN && scanline[idx + run] == byte {
        run += 1;
    }

    run
}

/// A repeat packet costs two bytes, so it only saves space for runs of three or
/// more - unless the run ends the scanline, where a two byte run is a wash but
/// lets us avoid emitting a literal packet header of its own.
fn is_worth_encoding_as_run(scanline: &[u8], idx: usize, run: usize) -> bool {
    run >= 3 || (run == 2 && idx + run == scanline.len())
}

#[cfg(test)]
mod tests {
    use crate::psd_channel::rle_decompress;

    use super::*;

    #[test]
    fn encodes_a_run() {
        assert_eq!(pack_bits(&[7; 5]), vec![252, 7]);
    }

    #[test]
    fn encodes_literals() {
        assert_eq!(pack_bits(&[1, 2, 3]), vec![2, 1, 2, 3]);
    }

    #[test]
    fn encodes_a_maximum_length_run() {
        let packed = pack_bits(&[9; 128]);

        assert_eq!(packed, vec![129, 9]);
    }

    #[test]
    fn encodes_a_maximum_length_literal() {
        let scanline: Vec<u8> = (0..130).map(|idx| (idx % 251) as u8).collect();
        let packed = pack_bits(&scanline);

        assert_eq!(packed[0], 127);
        assert_eq!(rle_decompress(&packed), scanline);
    }

    /// Round trip a handful of shapes through our encoder and the crate's
    /// decoder.
    #[test]
    fn round_trips_through_the_decoder() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0, 0],
            vec![1, 2, 2, 3, 3, 3, 4],
            vec![255; 300],
            (0..1000).map(|idx| (idx % 7) as u8).collect(),
            (0..1000)
                .map(|idx| if idx % 100 < 90 { 0 } else { 255 })
                .collect(),
        ];

        for case in cases {
            assert_eq!(rle_decompress(&pack_bits(&case)), case);
        }
    }

    /// Our scanline byte counts are written as u16s, so a compressed scanline
    /// must never grow past u16::MAX. The worst case is a scanline where no two
    /// neighboring bytes are equal.
    #[test]
    fn worst_case_expansion_fits_in_a_u16() {
        // The widest PSD that the file header allows.
        let scanline: Vec<u8> = (0..30_000).map(|idx| (idx % 251) as u8).collect();

        assert!(pack_bits(&scanline).len() <= u16::MAX as usize);
    }
}
