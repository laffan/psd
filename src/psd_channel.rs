use crate::sections::image_data_section::ChannelBytes;
use crate::sections::PsdCursor;
use thiserror::Error;

pub trait IntoRgba {
    /// Given an index of a pixel in the current rectangle
    /// (top left is 0.. to the right of that is 1.. etc) return the index of that pixel in the
    /// RGBA image that will be generated.
    ///
    /// If the final image or layer is the size of the PSD then this will return the same idx,
    /// otherwise it will get transformed.
    ///
    /// index could be `None` if layer's top or left is negative.
    ///
    /// index could be bigger than the size of the image if layer's bottom, right, width, height is bigger than image.
    fn rgba_idx(&self, idx: usize) -> Option<usize>;

    /// The first channel
    fn red(&self) -> &ChannelBytes;

    /// The second channel
    fn green(&self) -> Option<&ChannelBytes>;

    /// The third channel
    fn blue(&self) -> Option<&ChannelBytes>;

    /// The fourth channel
    fn alpha(&self) -> Option<&ChannelBytes>;

    /// The width of the PSD
    fn psd_width(&self) -> u32;

    /// The height of the PSD
    fn psd_height(&self) -> u32;

    /// Where this image's pixels sit in the PSD, as `(left, top, width,
    /// height)`, when every one of them lands inside it.
    ///
    /// That is what lets a channel be copied in a row at a time: a pixel's
    /// place in the RGBA buffer is then its row and column offset by the
    /// corner, with nothing to clip. `None` sends a channel down the general
    /// path, which maps each byte through [`IntoRgba::rgba_idx`] on its own.
    fn rect_inside_psd(&self) -> Option<(usize, usize, usize, usize)> {
        None
    }

    fn generate_rgba(&self) -> Vec<u8> {
        let rgba_len = (self.psd_width() * self.psd_height() * 4) as usize;

        let red = self.red();
        let green = self.green();
        let blue = self.blue();
        let alpha = self.alpha();

        // TODO: We're assuming that if we only see two channels it is a 16 bit grayscale
        // PSD. Instead we should just check the Psd's color mode and depth to see if
        // they are grayscale and sixteen. As we run into more cases we'll clean things like
        // this up over time.
        //        if green.is_some() && blue.is_none() && alpha.is_none() {
        //            return self.generate_16_bit_grayscale_rgba();
        //        }

        let mut rgba = vec![0; rgba_len];

        use crate::psd_channel::PsdChannelKind::*;

        self.insert_channel_bytes(&mut rgba, Red, red);

        // If there is a green channel we use it, otherwise we use the red channel since this is
        // a single channel grey image (such as a heightmap).
        if let Some(green) = green {
            self.insert_channel_bytes(&mut rgba, Green, green);
        } else {
            self.insert_channel_bytes(&mut rgba, Green, red);
        }

        // If there is a blue channel we use it, otherwise we use the red channel since this is
        // a single channel grey image (such as a heightmap).
        if let Some(blue) = blue {
            self.insert_channel_bytes(&mut rgba, Blue, blue);
        } else {
            self.insert_channel_bytes(&mut rgba, Blue, red);
        }

        if let Some(alpha_channel) = alpha {
            self.insert_channel_bytes(&mut rgba, TransparencyMask, alpha_channel);
        } else {
            // If there is no transparency data then the image is opaque
            for idx in 0..rgba_len / 4 {
                rgba[idx * 4 + 3] = 255;
            }
        }

        rgba
    }

    /// Generate an RGBA Vec<u8> from a composite image or layer that uses 16 bits per
    /// pixel. We do this by mapping the 16 bits back down to 8 bits.
    ///
    /// The 16 bits are stored across the red and green channels (first and second).
    fn generate_16_bit_grayscale_rgba(&self) -> Vec<u8> {
        match self.red() {
            ChannelBytes::RawData(red) => match self.green().unwrap() {
                ChannelBytes::RawData(green) => sixteen_to_eight_rgba(red, green),
                ChannelBytes::RleCompressed(green) => {
                    let green = &rle_decompress(green);

                    sixteen_to_eight_rgba(red, green)
                }
            },
            ChannelBytes::RleCompressed(red) => {
                let red = &rle_decompress(red);

                match self.green().unwrap() {
                    ChannelBytes::RawData(green) => sixteen_to_eight_rgba(red, green),
                    ChannelBytes::RleCompressed(green) => {
                        let green = &rle_decompress(green);
                        sixteen_to_eight_rgba(red, green)
                    }
                }
            }
        }
    }

    /// Given some vector of bytes, insert the bytes from the given channel into the vector.
    ///
    /// Doing it this way allows us to allocate for one vector and insert all 4 (RGBA) channels into
    /// it.
    fn insert_channel_bytes(
        &self,
        rgba: &mut Vec<u8>,
        channel_kind: PsdChannelKind,
        channel_bytes: &ChannelBytes,
    ) {
        if self.insert_channel_rows(rgba, channel_kind, channel_bytes) {
            return;
        }

        match channel_bytes {
            ChannelBytes::RawData(channel_bytes) => {
                let offset = channel_kind.rgba_offset().unwrap();

                for (idx, byte) in channel_bytes.iter().enumerate() {
                    if let Some(rgba_idx) = self.rgba_idx(idx) {
                        rgba[rgba_idx * 4 + offset] = *byte;
                    }
                }
            }
            // https://en.wikipedia.org/wiki/PackBits
            ChannelBytes::RleCompressed(channel_bytes) => {
                self.insert_rle_channel(rgba, channel_kind, &channel_bytes);
            }
        }
    }

    /// The same as [`IntoRgba::insert_channel_bytes`], a row at a time, for an
    /// image that sits wholly inside the PSD. Returns false, having written
    /// nothing, whenever the general path could answer differently — the
    /// image is not wholly inside, or its channel holds more bytes than its
    /// rectangle — so the caller can take that path instead.
    ///
    /// The general path works out a destination for every byte with a
    /// division and a remainder. For a layer the size of a 300 DPI page that
    /// is millions of them per channel, which made decoding a layer cost
    /// several times what parsing the whole file did.
    fn insert_channel_rows(
        &self,
        rgba: &mut [u8],
        channel_kind: PsdChannelKind,
        channel_bytes: &ChannelBytes,
    ) -> bool {
        let Some((left, top, width, height)) = self.rect_inside_psd() else {
            return false;
        };
        let Ok(offset) = channel_kind.rgba_offset() else {
            return false;
        };
        let psd_width = self.psd_width() as usize;
        if width == 0 || rgba.len() < (top + height) * psd_width * 4 {
            return false;
        }

        let decoded;
        let plane: &[u8] = match channel_bytes {
            ChannelBytes::RawData(bytes) => bytes,
            ChannelBytes::RleCompressed(bytes) => match rle_decompress_at_most(bytes, width * height) {
                Some(bytes) => {
                    decoded = bytes;
                    &decoded
                }
                None => return false,
            },
        };
        if plane.len() > width * height {
            return false;
        }

        for (row, scanline) in plane.chunks(width).enumerate() {
            let start = ((top + row) * psd_width + left) * 4 + offset;
            for (pixel, byte) in rgba[start..].iter_mut().step_by(4).zip(scanline) {
                *pixel = *byte;
            }
        }
        true
    }

    /// rle decompress a channel (R,G,B or A) and insert it into a vector of RGBA pixels.
    ///
    /// We use the channels offset to know where to put it.. So red would go in 0, 4, 8..
    /// blue would go in 1, 5, 9.. etc
    ///
    /// https://en.wikipedia.org/wiki/PackBits - algorithm used for decompression
    fn insert_rle_channel(
        &self,
        rgba: &mut Vec<u8>,
        channel_kind: PsdChannelKind,
        channel_bytes: &[u8],
    ) {
        let mut cursor = PsdCursor::new(&channel_bytes[..]);

        let mut idx = 0;
        let offset = channel_kind.rgba_offset().unwrap();
        let len = cursor.get_ref().len() as u64;

        while cursor.position() < len {
            let header = cursor.read_i8() as i16;

            if header == -128 {
                continue;
            } else if header >= 0 {
                let bytes_to_read = 1 + header;
                if cursor.position() + bytes_to_read as u64 > len {
                    break;
                }
                for byte in cursor.read(bytes_to_read as u32) {
                    if let Some(rgba_idx) = self.rgba_idx(idx) {
                        if let Some(buffer) = rgba.get_mut(rgba_idx * 4 + offset) {
                            *buffer = *byte;
                        }
                    }

                    idx += 1;
                }
            } else {
                let repeat = 1 - header;

                if cursor.position() + 1 > len {
                    break;
                }
                let byte = cursor.read_1()[0];
                for _ in 0..repeat {
                    if let Some(rgba_idx) = self.rgba_idx(idx) {
                        if let Some(buffer) = rgba.get_mut(rgba_idx * 4 + offset) {
                            *buffer = byte;
                        }
                    }

                    idx += 1;
                }
            };
        }
    }
}

/// PackBits-decode a channel the way [`IntoRgba::insert_rle_channel`] reads
/// one — the same packets, and the same stop at a packet the data cuts short
/// — into a buffer, or None if it would come to more than `limit` bytes.
fn rle_decompress_at_most(bytes: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(limit);
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes[at] as i8 as i16;
        at += 1;
        if header == -128 {
            continue;
        } else if header >= 0 {
            let count = 1 + header as usize;
            if at + count > bytes.len() {
                break;
            }
            if out.len() + count > limit {
                return None;
            }
            out.extend_from_slice(&bytes[at..at + count]);
            at += count;
        } else {
            let count = (1 - header) as usize;
            if at + 1 > bytes.len() {
                break;
            }
            if out.len() + count > limit {
                return None;
            }
            out.resize(out.len() + count, bytes[at]);
            at += 1;
        }
    }
    Some(out)
}

/// Rle decompress a channel
pub(crate) fn rle_decompress(bytes: &[u8]) -> Vec<u8> {
    let mut cursor = PsdCursor::new(&bytes[..]);

    let mut decompressed = vec![];

    while cursor.position() != cursor.get_ref().len() as u64 {
        let header = cursor.read_i8() as i16;

        if header == -128 {
            continue;
        } else if header >= 0 {
            let bytes_to_read = 1 + header;
            for byte in cursor.read(bytes_to_read as u32) {
                decompressed.push(*byte);
            }
        } else {
            let repeat = 1 - header;
            let byte = cursor.read_1()[0];
            for _ in 0..repeat {
                decompressed.push(byte);
            }
        };
    }

    decompressed
}

/// Take two 8 bit channels that together represent a 16 bit channel and convert them down
/// into an 8 bit channel.
///
/// We store the final bytes in the first channel (overwriting the old bytes)
fn sixteen_to_eight_rgba(channel1: &[u8], channel2: &[u8]) -> Vec<u8> {
    let mut eight = Vec::with_capacity(channel1.len());

    for idx in 0..channel1.len() {
        if idx % 2 == 1 {
            continue;
        }

        let sixteen_bit = [channel1[idx], channel1[idx + 1]];
        let sixteen_bit = u16::from_be_bytes(sixteen_bit);

        let eight_bit = (sixteen_bit / 256) as u8;

        eight.push(eight_bit);
        eight.push(eight_bit);
        eight.push(eight_bit);
        eight.push(255);
    }

    for idx in 0..channel2.len() {
        if idx % 2 == 1 {
            continue;
        }

        let sixteen_bit = [channel2[idx], channel2[idx + 1]];
        let sixteen_bit = u16::from_be_bytes(sixteen_bit);

        let eight_bit = (sixteen_bit / 256) as u8;

        eight.push(eight_bit);
        eight.push(eight_bit);
        eight.push(eight_bit);
        eight.push(255);
    }

    eight
}

/// Indicates how a channe'sl data is compressed
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum PsdChannelCompression {
    /// Not compressed
    RawData = 0,
    /// Compressed using [PackBits RLE compression](https://en.wikipedia.org/wiki/PackBits)
    RleCompressed = 1,
    /// Currently unsupported
    ZipWithoutPrediction = 2,
    /// Currently unsupported
    ZipWithPrediction = 3,
}

impl PsdChannelCompression {
    /// Create a new PsdLayerChannelCompression
    pub fn new(compression: u16) -> Option<PsdChannelCompression> {
        match compression {
            0 => Some(PsdChannelCompression::RawData),
            1 => Some(PsdChannelCompression::RleCompressed),
            2 => Some(PsdChannelCompression::ZipWithoutPrediction),
            3 => Some(PsdChannelCompression::ZipWithPrediction),
            _ => None,
        }
    }
}

/// The different kinds of channels in a layer (red, green, blue, ...).
#[derive(Debug, Hash, Eq, PartialEq, Ord, PartialOrd, Copy, Clone)]
#[allow(missing_docs)]
pub enum PsdChannelKind {
    Red = 0,
    Green = 1,
    Blue = 2,
    TransparencyMask = -1,
    UserSuppliedLayerMask = -2,
    RealUserSuppliedLayerMask = -3,
}

/// Represents an invalid channel
#[derive(Debug, Error)]
pub enum PsdChannelError {
    #[error("Channel {channel:#?} not present")]
    ChannelNotFound { channel: PsdChannelKind },
}

impl PsdChannelKind {
    /// Create a new PsdLayerChannel
    pub fn new(channel_id: i16) -> Option<PsdChannelKind> {
        match channel_id {
            0 => Some(PsdChannelKind::Red),
            1 => Some(PsdChannelKind::Green),
            2 => Some(PsdChannelKind::Blue),
            -1 => Some(PsdChannelKind::TransparencyMask),
            -2 => Some(PsdChannelKind::UserSuppliedLayerMask),
            -3 => Some(PsdChannelKind::RealUserSuppliedLayerMask),
            _ => None,
        }
    }

    /// R -> 0
    /// G -> 1
    /// B -> 2
    /// A -> 3
    pub fn rgba_offset(self) -> Result<usize, String> {
        match self {
            PsdChannelKind::Red => Ok(0),
            PsdChannelKind::Green => Ok(1),
            PsdChannelKind::Blue => Ok(2),
            PsdChannelKind::TransparencyMask => Ok(3),
            _ => Err(format!("{:#?} is not an RGBA channel", &self)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sections::layer_and_mask_information_section::layer::{
        BlendMode, LayerChannels, LayerProperties,
    };
    use crate::PsdLayer;

    use super::*;

    /// Verify that when inserting an RLE channel's bytes into an RGBA byte vec we do not attempt to
    /// read beyond the channel's length.
    #[test]
    fn does_not_read_beyond_rle_channels_bytes() {
        let layer_properties = LayerProperties {
            name: "".into(),
            layer_top: 0,
            layer_left: 0,
            layer_bottom: 0,
            layer_right: 0,
            visible: true,
            opacity: 0,
            clipping_mask: false,
            psd_width: 1,
            psd_height: 1,
            blend_mode: BlendMode::Normal,
            group_id: None,
        };

        let layer = PsdLayer {
            channels: LayerChannels::from([(
                PsdChannelKind::Red,
                ChannelBytes::RleCompressed(vec![0, 0, 0]),
            )]),
            layer_properties,
            layer_mask: None,
            vector_mask: None,
        };

        let mut rgba = vec![0; (layer.width() * layer.height() * 4) as usize];

        layer.insert_channel_bytes(&mut rgba, PsdChannelKind::Red, layer.red());

        assert_eq!(rgba, vec![0; 4]);
    }

    /// A layer inside the canvas decodes a row at a time; the answer is the
    /// one the per-byte path gives, RLE and raw, cut short or not.
    #[test]
    fn rows_are_what_single_bytes_were() {
        let (width, height) = (7u32, 5u32);
        let packed: Vec<u8> = (0..height)
            .flat_map(|row| {
                let line: Vec<u8> = (0..width).map(|col| ((row * 3 + col) % 4 * 60) as u8).collect();
                crate::write::pack_bits_for_tests(&line)
            })
            .collect();
        let raw: Vec<u8> = (0..width * height).map(|i| (i * 7 % 256) as u8).collect();
        let cases = vec![
            ChannelBytes::RleCompressed(packed.clone()),
            ChannelBytes::RleCompressed(packed[..packed.len() - 3].to_vec()),
            ChannelBytes::RawData(raw.clone()),
            ChannelBytes::RawData(raw[..10].to_vec()),
        ];
        for (left, top) in [(0, 0), (3, 2), (13, 10)] {
            for bytes in &cases {
                let layer = PsdLayer {
                    channels: LayerChannels::from([(PsdChannelKind::Red, bytes.clone())]),
                    layer_properties: LayerProperties {
                        name: "".into(),
                        layer_top: top,
                        layer_left: left,
                        layer_bottom: top + height as i32 - 1,
                        layer_right: left + width as i32 - 1,
                        visible: true,
                        opacity: 255,
                        clipping_mask: false,
                        psd_width: 20,
                        psd_height: 15,
                        blend_mode: BlendMode::Normal,
                        group_id: None,
                    },
                    layer_mask: None,
                    vector_mask: None,
                };
                assert!(layer.rect_inside_psd().is_some());

                let mut rows = vec![0; 20 * 15 * 4];
                assert!(layer.insert_channel_rows(&mut rows, PsdChannelKind::Red, bytes));
                let mut single = vec![0; 20 * 15 * 4];
                match bytes {
                    ChannelBytes::RleCompressed(b) => {
                        layer.insert_rle_channel(&mut single, PsdChannelKind::Red, b)
                    }
                    ChannelBytes::RawData(b) => {
                        for (idx, byte) in b.iter().enumerate() {
                            single[layer.rgba_idx(idx).unwrap() * 4] = *byte;
                        }
                    }
                }
                assert_eq!(rows, single);
            }
        }
    }
}
