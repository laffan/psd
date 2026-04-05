use psd::Psd;

const FIXTURE: &[u8] = include_bytes!("./fixtures/vector-mask-and-layer-mask.psd");

#[test]
fn parse_psd_with_vector_mask_and_layer_mask() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    assert_eq!(psd.width(), 100);
    assert_eq!(psd.height(), 100);
    assert_eq!(psd.layers().len(), 2);
}

// --- Vector Mask Tests ---

#[test]
fn shape_layer_has_vector_mask() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    // Layers are stored in reverse order in PSD files; after reversal,
    // "Masked Layer" is first (bottom), "Shape Layer" is second (top)
    let shape_layer = psd.layer_by_name("Shape Layer").unwrap();
    assert!(shape_layer.has_vector_mask());
}

#[test]
fn vector_mask_has_correct_subpaths() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let shape_layer = psd.layer_by_name("Shape Layer").unwrap();
    let vmask = shape_layer.vector_mask().unwrap();

    assert!(!vmask.invert);
    assert!(!vmask.not_linked);
    assert!(!vmask.disabled);

    // Should have 1 closed rectangular subpath
    assert_eq!(vmask.paths.len(), 1);
    let subpath = &vmask.paths[0];
    assert!(subpath.closed);
    assert_eq!(subpath.knots.len(), 4);
}

#[test]
fn vector_mask_anchor_coordinates_are_correct() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let shape_layer = psd.layer_by_name("Shape Layer").unwrap();
    let vmask = shape_layer.vector_mask().unwrap();
    let knots = &vmask.paths[0].knots;

    // Rectangle from 25% to 75% of the document
    // Knot 0: top-left (0.25, 0.25)
    assert!((knots[0].anchor.x - 0.25).abs() < 0.001);
    assert!((knots[0].anchor.y - 0.25).abs() < 0.001);

    // Knot 1: top-right (0.75, 0.25)
    assert!((knots[1].anchor.x - 0.75).abs() < 0.001);
    assert!((knots[1].anchor.y - 0.25).abs() < 0.001);

    // Knot 2: bottom-right (0.75, 0.75)
    assert!((knots[2].anchor.x - 0.75).abs() < 0.001);
    assert!((knots[2].anchor.y - 0.75).abs() < 0.001);

    // Knot 3: bottom-left (0.25, 0.75)
    assert!((knots[3].anchor.x - 0.25).abs() < 0.001);
    assert!((knots[3].anchor.y - 0.75).abs() < 0.001);
}

#[test]
fn vector_mask_pixel_coordinate_conversion() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let shape_layer = psd.layer_by_name("Shape Layer").unwrap();
    let vmask = shape_layer.vector_mask().unwrap();
    let anchor = &vmask.paths[0].knots[0].anchor;

    // Convert normalized coords to pixel coords for a 100x100 document
    let pixel_x = anchor.x * psd.width() as f64;
    let pixel_y = anchor.y * psd.height() as f64;

    assert!((pixel_x - 25.0).abs() < 0.1);
    assert!((pixel_y - 25.0).abs() < 0.1);
}

#[test]
fn layer_without_vector_mask_returns_none() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let masked_layer = psd.layer_by_name("Masked Layer").unwrap();
    assert!(!masked_layer.has_vector_mask());
    assert!(masked_layer.vector_mask().is_none());
}

// --- Layer Mask Tests ---

#[test]
fn masked_layer_has_mask() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let masked_layer = psd.layer_by_name("Masked Layer").unwrap();
    assert!(masked_layer.mask().is_some());
}

#[test]
fn layer_mask_has_correct_bbox() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let masked_layer = psd.layer_by_name("Masked Layer").unwrap();
    let mask = masked_layer.mask().unwrap();

    assert_eq!(mask.top, 30);
    assert_eq!(mask.left, 30);
    assert_eq!(mask.bottom, 70);
    assert_eq!(mask.right, 70);
    assert_eq!(mask.width(), 40);
    assert_eq!(mask.height(), 40);
}

#[test]
fn layer_mask_default_flags() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let masked_layer = psd.layer_by_name("Masked Layer").unwrap();
    let mask = masked_layer.mask().unwrap();

    assert_eq!(mask.default_color, 0);
    assert!(!mask.disabled);
    assert!(!mask.invert);
    assert!(!mask.relative_to_layer);
}

#[test]
fn mask_pixels_returns_correct_data() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let masked_layer = psd.layer_by_name("Masked Layer").unwrap();
    let mask = masked_layer.mask().unwrap();
    let pixels = masked_layer.mask_pixels().unwrap();

    let expected_len = mask.width() as usize * mask.height() as usize;
    assert_eq!(pixels.len(), expected_len);

    // All mask pixels should be 200 (gray)
    assert!(pixels.iter().all(|&b| b == 200));
}

#[test]
fn layer_without_mask_returns_none() {
    let psd = Psd::from_bytes(FIXTURE).unwrap();
    let shape_layer = psd.layer_by_name("Shape Layer").unwrap();
    assert!(shape_layer.mask().is_none());
    assert!(shape_layer.mask_pixels().is_none());
}
