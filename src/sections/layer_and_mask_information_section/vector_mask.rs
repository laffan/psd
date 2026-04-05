use crate::sections::PsdCursor;

/// A vector mask attached to a layer, containing Bezier path data.
///
/// Vector masks define shape outlines using Bezier curves. The path coordinates
/// are normalized to the 0.0–1.0 range relative to the document dimensions.
///
/// # Example
///
/// ```ignore
/// let psd = Psd::from_bytes(psd_bytes).unwrap();
/// for layer in psd.layers() {
///     if let Some(mask) = layer.vector_mask() {
///         for subpath in &mask.paths {
///             for knot in &subpath.knots {
///                 let pixel_x = knot.anchor.x * psd.width() as f64;
///                 let pixel_y = knot.anchor.y * psd.height() as f64;
///                 println!("anchor at ({}, {})", pixel_x, pixel_y);
///             }
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct VectorMask {
    /// If true, the mask is inverted.
    pub invert: bool,
    /// If true, the mask is not linked to the layer.
    pub not_linked: bool,
    /// If true, the mask is disabled.
    pub disabled: bool,
    /// The subpaths that make up this vector mask.
    pub paths: Vec<Subpath>,
}

/// A subpath within a vector mask, consisting of a sequence of Bezier knots.
#[derive(Debug, Clone)]
pub struct Subpath {
    /// If true, this is a closed subpath (the last knot connects back to the first).
    pub closed: bool,
    /// The Bezier knots that define this subpath.
    pub knots: Vec<BezierKnot>,
}

/// A Bezier knot with preceding control point, anchor point, and following control point.
#[derive(Debug, Clone)]
pub struct BezierKnot {
    /// The control point preceding the anchor (used for the curve entering the anchor).
    pub preceding: PathPoint,
    /// The anchor point itself.
    pub anchor: PathPoint,
    /// The control point following the anchor (used for the curve leaving the anchor).
    pub following: PathPoint,
    /// If true, the control points are linked to the anchor.
    pub linked: bool,
}

/// A point in normalized coordinates (0.0–1.0 relative to document dimensions).
///
/// Multiply `x` by the document width and `y` by the document height to get pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct PathPoint {
    /// Horizontal position, normalized to 0.0–1.0. Multiply by image width for pixels.
    pub x: f64,
    /// Vertical position, normalized to 0.0–1.0. Multiply by image height for pixels.
    pub y: f64,
}

/// Fixed-point 8.24 divisor for converting raw path coordinates to normalized 0.0–1.0 range.
const FIXED_POINT_DIVISOR: f64 = 16777216.0; // 2^24

/// Parse a vector mask from a vmsk/vsms additional layer information block.
///
/// The `data` slice should start at the beginning of the block's data (after key and length).
pub(crate) fn parse_vector_mask(data: &[u8]) -> Option<VectorMask> {
    if data.len() < 8 {
        return None;
    }

    let mut cursor = PsdCursor::new(data);

    // Version (4 bytes, should be 3)
    let _version = cursor.read_u32();

    // Flags (4 bytes)
    let flags = cursor.read_u32();
    let invert = flags & 1 != 0;
    let not_linked = flags & 2 != 0;
    let disabled = flags & 4 != 0;

    let remaining = data.len() as u64 - cursor.position();
    if remaining % 26 != 0 {
        // Not an exact multiple of path record size — try to parse what we can
    }

    let record_count = remaining / 26;
    let mut paths: Vec<Subpath> = Vec::new();
    let mut current_subpath: Option<(bool, Vec<BezierKnot>)> = None;

    for _ in 0..record_count {
        let selector = cursor.read_u16();
        let record_data = cursor.read(24);

        match selector {
            // Closed subpath length record
            0 => {
                // Finish any previous subpath
                if let Some((closed, knots)) = current_subpath.take() {
                    paths.push(Subpath { closed, knots });
                }
                current_subpath = Some((true, Vec::new()));
            }
            // Open subpath length record
            3 => {
                if let Some((closed, knots)) = current_subpath.take() {
                    paths.push(Subpath { closed, knots });
                }
                current_subpath = Some((false, Vec::new()));
            }
            // Bezier knot records: 1=closed linked, 2=closed unlinked, 4=open linked, 5=open unlinked
            1 | 2 | 4 | 5 => {
                let linked = selector == 1 || selector == 4;
                let knot = parse_bezier_knot(record_data, linked);
                if let Some((_, ref mut knots)) = current_subpath {
                    knots.push(knot);
                }
            }
            // 6 = path fill rule, 7 = clipboard record — ignore
            _ => {}
        }
    }

    // Finish the last subpath
    if let Some((closed, knots)) = current_subpath.take() {
        paths.push(Subpath { closed, knots });
    }

    Some(VectorMask {
        invert,
        not_linked,
        disabled,
        paths,
    })
}

/// Parse a 24-byte Bezier knot record into a BezierKnot.
///
/// Layout: preceding(y,x), anchor(y,x), following(y,x) — each coordinate is i32 big-endian.
fn parse_bezier_knot(data: &[u8], linked: bool) -> BezierKnot {
    let preceding_y = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let preceding_x = i32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let anchor_y = i32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let anchor_x = i32::from_be_bytes([data[12], data[13], data[14], data[15]]);
    let following_y = i32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let following_x = i32::from_be_bytes([data[20], data[21], data[22], data[23]]);

    BezierKnot {
        preceding: PathPoint {
            x: preceding_x as f64 / FIXED_POINT_DIVISOR,
            y: preceding_y as f64 / FIXED_POINT_DIVISOR,
        },
        anchor: PathPoint {
            x: anchor_x as f64 / FIXED_POINT_DIVISOR,
            y: anchor_y as f64 / FIXED_POINT_DIVISOR,
        },
        following: PathPoint {
            x: following_x as f64 / FIXED_POINT_DIVISOR,
            y: following_y as f64 / FIXED_POINT_DIVISOR,
        },
        linked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal vmsk block with a single closed rectangular subpath (4 knots).
    fn make_vmsk_rect_bytes(coords: [(i32, i32); 4]) -> Vec<u8> {
        let mut data = Vec::new();

        // Version = 3
        data.extend_from_slice(&3u32.to_be_bytes());
        // Flags = 0
        data.extend_from_slice(&0u32.to_be_bytes());

        // Path fill rule record (selector=6, 24 zero bytes)
        data.extend_from_slice(&6u16.to_be_bytes());
        data.extend_from_slice(&[0u8; 24]);

        // Closed subpath length record (selector=0, knot count in first 2 bytes)
        data.extend_from_slice(&0u16.to_be_bytes());
        let mut length_data = [0u8; 24];
        length_data[0..2].copy_from_slice(&4u16.to_be_bytes());
        data.extend_from_slice(&length_data);

        // 4 closed linked knot records (selector=1)
        for (y, x) in &coords {
            data.extend_from_slice(&1u16.to_be_bytes());
            // preceding = anchor = following for a straight-line rectangle
            for _ in 0..3 {
                data.extend_from_slice(&y.to_be_bytes());
                data.extend_from_slice(&x.to_be_bytes());
            }
        }

        data
    }

    #[test]
    fn parse_rectangular_vector_mask() {
        // A rectangle at (0.25, 0.25) to (0.75, 0.75) in normalized coords
        let quarter = (FIXED_POINT_DIVISOR * 0.25) as i32;
        let three_quarter = (FIXED_POINT_DIVISOR * 0.75) as i32;

        let coords = [
            (quarter, quarter),           // top-left
            (quarter, three_quarter),     // top-right
            (three_quarter, three_quarter), // bottom-right
            (three_quarter, quarter),     // bottom-left
        ];

        let data = make_vmsk_rect_bytes(coords);
        let mask = parse_vector_mask(&data).expect("should parse");

        assert!(!mask.invert);
        assert!(!mask.not_linked);
        assert!(!mask.disabled);
        assert_eq!(mask.paths.len(), 1);

        let subpath = &mask.paths[0];
        assert!(subpath.closed);
        assert_eq!(subpath.knots.len(), 4);

        // Check first knot (top-left)
        let knot = &subpath.knots[0];
        assert!(knot.linked);
        assert!((knot.anchor.x - 0.25).abs() < 0.001);
        assert!((knot.anchor.y - 0.25).abs() < 0.001);

        // Check third knot (bottom-right)
        let knot = &subpath.knots[2];
        assert!((knot.anchor.x - 0.75).abs() < 0.001);
        assert!((knot.anchor.y - 0.75).abs() < 0.001);
    }

    #[test]
    fn parse_vector_mask_flags() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_be_bytes()); // version
        data.extend_from_slice(&7u32.to_be_bytes()); // flags: invert | not_linked | disabled

        let mask = parse_vector_mask(&data).expect("should parse");
        assert!(mask.invert);
        assert!(mask.not_linked);
        assert!(mask.disabled);
        assert!(mask.paths.is_empty());
    }

    #[test]
    fn parse_open_subpath() {
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_be_bytes()); // version
        data.extend_from_slice(&0u32.to_be_bytes()); // flags

        // Open subpath length record (selector=3)
        data.extend_from_slice(&3u16.to_be_bytes());
        let mut length_data = [0u8; 24];
        length_data[0..2].copy_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&length_data);

        // Two open unlinked knots (selector=5)
        let half = (FIXED_POINT_DIVISOR * 0.5) as i32;
        for _ in 0..2 {
            data.extend_from_slice(&5u16.to_be_bytes());
            for _ in 0..3 {
                data.extend_from_slice(&half.to_be_bytes());
                data.extend_from_slice(&half.to_be_bytes());
            }
        }

        let mask = parse_vector_mask(&data).expect("should parse");
        assert_eq!(mask.paths.len(), 1);
        assert!(!mask.paths[0].closed);
        assert_eq!(mask.paths[0].knots.len(), 2);
        assert!(!mask.paths[0].knots[0].linked);
    }

    #[test]
    fn pixel_coordinate_conversion() {
        // Test that normalized coordinates convert correctly to pixel space
        let half = (FIXED_POINT_DIVISOR * 0.5) as i32;
        let mut data = Vec::new();
        data.extend_from_slice(&3u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());

        // One closed subpath with one knot
        data.extend_from_slice(&0u16.to_be_bytes());
        let mut length_data = [0u8; 24];
        length_data[0..2].copy_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&length_data);

        data.extend_from_slice(&1u16.to_be_bytes());
        for _ in 0..3 {
            data.extend_from_slice(&half.to_be_bytes());
            data.extend_from_slice(&half.to_be_bytes());
        }

        let mask = parse_vector_mask(&data).unwrap();
        let knot = &mask.paths[0].knots[0];

        // For a 100x200 document, (0.5, 0.5) should map to (50, 100)
        let doc_width = 100.0_f64;
        let doc_height = 200.0_f64;
        let pixel_x = knot.anchor.x * doc_width;
        let pixel_y = knot.anchor.y * doc_height;

        assert!((pixel_x - 50.0).abs() < 0.01);
        assert!((pixel_y - 100.0).abs() < 0.01);
    }
}
