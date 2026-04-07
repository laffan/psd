use crate::sections::PsdCursor;

/// Metadata about a raster mask attached to a layer.
///
/// The mask's pixel data is stored separately as a channel (channel ID -2 or -3)
/// and can be retrieved via `PsdLayer::mask_pixels()`.
///
/// # Example
///
/// ```ignore
/// let psd = Psd::from_bytes(psd_bytes).unwrap();
/// for layer in psd.layers() {
///     if let Some(mask) = layer.mask() {
///         let w = mask.right - mask.left;
///         let h = mask.bottom - mask.top;
///         println!("mask bbox: {}x{} at ({}, {})", w, h, mask.left, mask.top);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct LayerMask {
    /// Top edge of the mask bounding rectangle.
    pub top: i32,
    /// Left edge of the mask bounding rectangle.
    pub left: i32,
    /// Bottom edge of the mask bounding rectangle.
    pub bottom: i32,
    /// Right edge of the mask bounding rectangle.
    pub right: i32,
    /// Default color of the mask (0 or 255).
    pub default_color: u8,
    /// If true, the mask is disabled.
    pub disabled: bool,
    /// If true, the mask should be inverted when blending.
    pub invert: bool,
    /// If true, the mask position is relative to the layer.
    pub relative_to_layer: bool,
}

impl LayerMask {
    /// Width of the mask in pixels.
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }

    /// Height of the mask in pixels.
    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }
}

/// Parse layer mask data from the layer record's mask data section.
///
/// Returns `None` if the mask data length is 0 (no mask present).
pub(crate) fn parse_layer_mask(cursor: &mut PsdCursor) -> Option<LayerMask> {
    let mask_data_len = cursor.read_u32();

    if mask_data_len == 0 {
        return None;
    }

    let start_pos = cursor.position();

    // Read bounding rectangle
    let top = cursor.read_i32();
    let left = cursor.read_i32();
    let bottom = cursor.read_i32();
    let right = cursor.read_i32();

    // Default color (0 or 255)
    let default_color = cursor.read_u8();

    // Flags byte
    let flags = cursor.read_u8();
    let relative_to_layer = flags & 1 != 0;
    let disabled = flags & (1 << 1) != 0;
    let invert = flags & (1 << 2) != 0;

    // Skip remaining bytes (real flags, real bbox, mask parameters, padding, etc.)
    let consumed = (cursor.position() - start_pos) as u32;
    if consumed < mask_data_len {
        cursor.read(mask_data_len - consumed);
    }

    Some(LayerMask {
        top,
        left,
        bottom,
        right,
        default_color,
        disabled,
        invert,
        relative_to_layer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mask_bytes(top: i32, left: i32, bottom: i32, right: i32, default_color: u8, flags: u8) -> Vec<u8> {
        let mut data = Vec::new();
        // Size = 20 bytes (4*4 bbox + 1 default_color + 1 flags + 2 padding)
        data.extend_from_slice(&20u32.to_be_bytes());
        data.extend_from_slice(&top.to_be_bytes());
        data.extend_from_slice(&left.to_be_bytes());
        data.extend_from_slice(&bottom.to_be_bytes());
        data.extend_from_slice(&right.to_be_bytes());
        data.push(default_color);
        data.push(flags);
        // Padding to reach 20 bytes (20 - 18 = 2 bytes)
        data.push(0);
        data.push(0);
        data
    }

    #[test]
    fn parse_basic_layer_mask() {
        let bytes = make_mask_bytes(10, 20, 110, 220, 0, 0);
        let mut cursor = PsdCursor::new(&bytes);
        let mask = parse_layer_mask(&mut cursor).expect("should parse");

        assert_eq!(mask.top, 10);
        assert_eq!(mask.left, 20);
        assert_eq!(mask.bottom, 110);
        assert_eq!(mask.right, 220);
        assert_eq!(mask.default_color, 0);
        assert!(!mask.disabled);
        assert!(!mask.invert);
        assert!(!mask.relative_to_layer);
        assert_eq!(mask.width(), 200);
        assert_eq!(mask.height(), 100);
    }

    #[test]
    fn parse_mask_with_flags() {
        // flags: bit 0 = relative, bit 1 = disabled, bit 2 = invert => 0b111 = 7
        let bytes = make_mask_bytes(0, 0, 50, 50, 255, 7);
        let mut cursor = PsdCursor::new(&bytes);
        let mask = parse_layer_mask(&mut cursor).unwrap();

        assert!(mask.relative_to_layer);
        assert!(mask.disabled);
        assert!(mask.invert);
        assert_eq!(mask.default_color, 255);
    }

    #[test]
    fn parse_no_mask() {
        let bytes = 0u32.to_be_bytes();
        let mut cursor = PsdCursor::new(&bytes);
        assert!(parse_layer_mask(&mut cursor).is_none());
    }
}
