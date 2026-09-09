//! Tests for creating new PSD files.
//!
//! Everything that we write here gets parsed back with `Psd::from_bytes` so
//! that the crate's reader is the judge of whether our writer produced a valid
//! file.

use psd::{
    BlendMode, ColorMode, GroupBuilder, LayerBuilder, Psd, PsdBuilder, PsdChannelCompression,
    PsdDepth, PsdWriteError,
};

/// `width * height` pixels of a single colour.
fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    rgba.iter()
        .copied()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// The file header we write describes an 8 bit RGB document of the size that
/// was asked for.
#[test]
fn writes_a_readable_file_header() {
    let mut builder = PsdBuilder::new(17, 23);
    builder.add_layer(LayerBuilder::new("Only").rgba(17, 23, solid(17, 23, GREEN)));

    let bytes = builder.to_bytes().unwrap();

    assert_eq!(&bytes[0..4], b"8BPS");
    assert_eq!(&bytes[4..6], &[0, 1]);
    assert_eq!(&bytes[6..12], &[0; 6]);

    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.width(), 17);
    assert_eq!(psd.height(), 23);
    assert_eq!(psd.depth(), PsdDepth::Eight);
    assert_eq!(psd.color_mode(), ColorMode::Rgb);
}

/// A single layer's pixels survive the round trip.
#[test]
fn round_trips_a_single_layer() {
    let mut builder = PsdBuilder::new(2, 2);
    builder.add_layer(LayerBuilder::new("First Layer").rgba(2, 2, solid(2, 2, GREEN)));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 1);
    assert_eq!(psd.groups().len(), 0);

    let layer = psd.layer_by_name("First Layer").unwrap();

    assert_eq!(layer.width(), 2);
    assert_eq!(layer.height(), 2);
    assert_eq!(layer.rgba(), solid(2, 2, GREEN));
    assert_eq!(psd.rgba(), solid(2, 2, GREEN));
}

/// Layers are added bottom first, and come back out of the reader top first.
#[test]
fn writes_layers_bottom_up() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(LayerBuilder::new("Bottom").rgba(1, 1, GREEN.to_vec()))
        .add_layer(LayerBuilder::new("Middle").rgba(1, 1, RED.to_vec()))
        .add_layer(LayerBuilder::new("Top").rgba(1, 1, BLUE.to_vec()));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let names: Vec<&str> = psd.layers().iter().map(|layer| layer.name()).collect();
    assert_eq!(names, vec!["Top", "Middle", "Bottom"]);

    // The top layer is opaque, so it is what the flattened image shows.
    assert_eq!(psd.rgba(), BLUE.to_vec());
}

/// Every property that we can set on a layer comes back the way we set it.
#[test]
fn round_trips_layer_properties() {
    let mut builder = PsdBuilder::new(10, 10);
    builder
        .add_layer(
            LayerBuilder::new("Visible")
                .rgba(4, 3, solid(4, 3, GREEN))
                .at(2, 5)
                .opacity(128)
                .blend_mode(BlendMode::Multiply),
        )
        .add_layer(
            LayerBuilder::new("Hidden")
                .rgba(1, 1, RED.to_vec())
                .visible(false),
        );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let visible = psd.layer_by_name("Visible").unwrap();
    assert_eq!(visible.visible(), true);
    assert_eq!(visible.opacity(), 128);
    assert_eq!(visible.blend_mode(), BlendMode::Multiply);
    assert_eq!(visible.layer_left(), 2);
    assert_eq!(visible.layer_top(), 5);
    assert_eq!(visible.layer_right(), 5);
    assert_eq!(visible.layer_bottom(), 7);
    assert_eq!(visible.width(), 4);
    assert_eq!(visible.height(), 3);

    let hidden = psd.layer_by_name("Hidden").unwrap();
    assert_eq!(hidden.visible(), false);
    assert_eq!(hidden.opacity(), 255);
    assert_eq!(hidden.blend_mode(), BlendMode::Normal);
}

/// A layer that only covers part of the canvas lands in the right place.
#[test]
fn positions_a_layer_within_the_canvas() {
    let mut builder = PsdBuilder::new(3, 3);
    builder.add_layer(
        LayerBuilder::new("Corner")
            .rgba(1, 1, BLUE.to_vec())
            .at(2, 1),
    );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let mut expected = vec![0; 3 * 3 * 4];
    // Row 1, column 2.
    expected[(1 * 3 + 2) * 4..(1 * 3 + 2) * 4 + 4].copy_from_slice(&BLUE);

    assert_eq!(psd.layer_by_name("Corner").unwrap().rgba(), expected);

    // The flattened image is stored composited over white, so the pixels that
    // the layer doesn't cover are transparent white rather than transparent
    // black. Photoshop writes them the same way.
    let mut expected_flattened = vec![255; 3 * 3 * 4];
    for pixel in expected_flattened.chunks_exact_mut(4) {
        pixel[3] = 0;
    }
    expected_flattened[(1 * 3 + 2) * 4..(1 * 3 + 2) * 4 + 4].copy_from_slice(&BLUE);

    assert_eq!(psd.rgba(), expected_flattened);
}

/// Photoshop stores a transparent document's flattened image already
/// composited over white. `tests/fixtures/blending/blue-red-1x1-normal.psd`
/// holds `[127, 63, 191, 192]` for a pixel whose true colour is
/// `[85, 0, 170, 192]`, so that's the shape that we write too.
#[test]
fn mattes_the_flattened_image_over_white() {
    let mut builder = PsdBuilder::new(2, 1);
    builder.flattened_image(vec![85, 0, 170, 192, 10, 20, 30, 0]);

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.rgba(), vec![127, 63, 191, 192, 255, 255, 255, 0]);
}

/// A layer inside of a group knows which group it is in.
#[test]
fn round_trips_a_group() {
    let mut builder = PsdBuilder::new(1, 1);
    builder.add_group(
        GroupBuilder::new("group").add_layer(LayerBuilder::new("Inside").rgba(
            1,
            1,
            GREEN.to_vec(),
        )),
    );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 1);
    assert_eq!(psd.groups().len(), 1);

    let group = psd.groups().get(&1).unwrap();
    assert_eq!(group.name(), "group");
    assert_eq!(group.parent_id(), None);
    assert_eq!(group.contained_layers(), 0..1);

    let layer = psd.layer_by_name("Inside").unwrap();
    assert_eq!(layer.parent_id(), Some(group.id()));
}

/// Groups nest, and the layers inside of them point at the innermost group.
///
/// ```text
/// group outside
///     group inside
///         Deep Layer
///     Shallow Layer
/// Root Layer
/// ```
#[test]
fn round_trips_nested_groups() {
    let inside = GroupBuilder::new("group inside").add_layer(LayerBuilder::new("Deep Layer").rgba(
        1,
        1,
        GREEN.to_vec(),
    ));

    let outside = GroupBuilder::new("group outside")
        .add_layer(LayerBuilder::new("Shallow Layer").rgba(1, 1, RED.to_vec()))
        .add_group(inside);

    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(LayerBuilder::new("Root Layer").rgba(1, 1, BLUE.to_vec()))
        .add_group(outside);

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 3);
    assert_eq!(psd.groups().len(), 2);

    let outside = psd
        .groups()
        .values()
        .find(|group| group.name() == "group outside")
        .unwrap();
    let inside = psd
        .groups()
        .values()
        .find(|group| group.name() == "group inside")
        .unwrap();

    assert_eq!(outside.parent_id(), None);
    assert_eq!(inside.parent_id(), Some(outside.id()));

    assert_eq!(
        psd.layer_by_name("Deep Layer").unwrap().parent_id(),
        Some(inside.id())
    );
    assert_eq!(
        psd.layer_by_name("Shallow Layer").unwrap().parent_id(),
        Some(outside.id())
    );
    assert_eq!(psd.layer_by_name("Root Layer").unwrap().parent_id(), None);

    // Every layer in the outer group, at any depth, is in its range.
    let contained: Vec<&str> = outside
        .contained_layers()
        .map(|idx| psd.layer_by_idx(idx).name())
        .collect();
    assert_eq!(contained, vec!["Deep Layer", "Shallow Layer"]);
}

/// Three groups deep, to be sure that nothing about nesting is hard coded to
/// one level.
#[test]
fn round_trips_deeply_nested_groups() {
    let mut group = GroupBuilder::new("depth 4").add_layer(LayerBuilder::new("Deepest").rgba(
        1,
        1,
        GREEN.to_vec(),
    ));

    for depth in (1..=3).rev() {
        group = GroupBuilder::new(format!("depth {}", depth)).add_group(group);
    }

    let mut builder = PsdBuilder::new(1, 1);
    builder.add_group(group);

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.groups().len(), 4);
    assert_eq!(psd.layers().len(), 1);

    // Walk from the layer back up to the root, collecting group names.
    let mut names = vec![];
    let mut parent = psd.layer_by_name("Deepest").unwrap().parent_id();
    while let Some(id) = parent {
        let group = psd.groups().get(&id).unwrap();
        names.push(group.name().to_string());
        parent = group.parent_id();
    }

    assert_eq!(names, vec!["depth 4", "depth 3", "depth 2", "depth 1"]);
}

/// A group's own settings live on the record that opens the folder, so they
/// have to survive the round trip too.
#[test]
fn round_trips_group_properties() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_group(
            GroupBuilder::new("dimmed")
                .opacity(64)
                .blend_mode(BlendMode::Screen)
                .add_layer(LayerBuilder::new("A").rgba(1, 1, GREEN.to_vec())),
        )
        .add_group(
            GroupBuilder::new("hidden")
                .visible(false)
                .collapsed(true)
                .add_layer(LayerBuilder::new("B").rgba(1, 1, RED.to_vec())),
        );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let dimmed = psd
        .groups()
        .values()
        .find(|group| group.name() == "dimmed")
        .unwrap();
    assert_eq!(dimmed.opacity(), 64);
    assert_eq!(dimmed.visible(), true);
    assert_eq!(dimmed.blend_mode(), BlendMode::Screen);

    let hidden = psd
        .groups()
        .values()
        .find(|group| group.name() == "hidden")
        .unwrap();
    assert_eq!(hidden.visible(), false);
    assert_eq!(hidden.opacity(), 255);
    assert_eq!(hidden.blend_mode(), BlendMode::PassThrough);
}

/// An empty group is two records with nothing in between them.
#[test]
fn round_trips_an_empty_group() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(LayerBuilder::new("Only Layer").rgba(1, 1, GREEN.to_vec()))
        .add_group(GroupBuilder::new("empty"));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 1);
    assert_eq!(psd.groups().len(), 1);

    let group = psd.groups().get(&1).unwrap();
    assert_eq!(group.name(), "empty");
    assert_eq!(group.contained_layers().len(), 0);
}

/// A layer with no pixels at all is a valid record.
#[test]
fn round_trips_a_layer_with_no_pixels() {
    let mut builder = PsdBuilder::new(1, 1);
    builder.add_layer(LayerBuilder::new("Empty Layer"));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 1);
    assert_eq!(
        psd.layer_by_name("Empty Layer").unwrap().name(),
        "Empty Layer"
    );
}

/// A PSD does not need to have any layers.
#[test]
fn writes_a_psd_with_no_layers() {
    let mut builder = PsdBuilder::new(2, 1);
    builder.flattened_image(solid(2, 1, RED));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 0);
    assert_eq!(psd.rgba(), solid(2, 1, RED));
}

/// Both of the compressions that we support describe the same pixels.
#[test]
fn raw_and_rle_describe_the_same_pixels() {
    // A gradient so that there is something for RLE to actually compress, plus
    // runs of identical bytes at the edges.
    let width = 40;
    let height = 9;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for column in 0..width {
            let shade = if column < 20 { 200 } else { (row * 20) as u8 };
            pixels.extend_from_slice(&[shade, 255 - shade, shade / 2, 255]);
        }
    }

    let mut file_sizes = vec![];
    for compression in [
        PsdChannelCompression::RawData,
        PsdChannelCompression::RleCompressed,
    ] {
        let mut builder = PsdBuilder::new(width as u32, height as u32);
        builder
            .compression(compression)
            .add_layer(LayerBuilder::new("Gradient").rgba(
                width as u32,
                height as u32,
                pixels.clone(),
            ));

        let bytes = builder.to_bytes().unwrap();
        let psd = Psd::from_bytes(&bytes).unwrap();

        assert_eq!(psd.layer_by_name("Gradient").unwrap().rgba(), pixels);
        assert_eq!(psd.rgba(), pixels);

        file_sizes.push(bytes.len());
    }

    // RLE should have paid for itself on an image with this many flat runs.
    assert!(
        file_sizes[1] < file_sizes[0],
        "expected the RLE file ({} bytes) to be smaller than the raw one ({} bytes)",
        file_sizes[1],
        file_sizes[0]
    );
}

/// The flattened image that we generate composites the layer stack.
#[test]
fn composites_the_flattened_image() {
    let mut builder = PsdBuilder::new(2, 1);
    builder
        .add_layer(LayerBuilder::new("Bottom").rgba(2, 1, solid(2, 1, RED)))
        // Covers only the right hand pixel.
        .add_layer(LayerBuilder::new("Top").rgba(1, 1, GREEN.to_vec()).at(1, 0));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let mut expected = RED.to_vec();
    expected.extend_from_slice(&GREEN);

    assert_eq!(psd.rgba(), expected);
}

/// Hidden layers stay out of the flattened image, and group opacity multiplies
/// into the layers inside of it.
#[test]
fn flattened_image_respects_visibility_and_opacity() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(
            LayerBuilder::new("Hidden")
                .rgba(1, 1, RED.to_vec())
                .visible(false),
        )
        .add_group(
            GroupBuilder::new("half")
                .opacity(128)
                .add_layer(LayerBuilder::new("White").rgba(1, 1, vec![255, 255, 255, 255])),
        );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let flattened = psd.rgba();

    // Nothing below the group, so we end up with half transparent white.
    assert_eq!(flattened[0..3], [255, 255, 255]);
    assert!((127..=129).contains(&flattened[3]));
}

/// A document with transparency in it gets an alpha channel, an opaque one
/// does not. This is what Photoshop does.
#[test]
fn only_writes_an_alpha_channel_when_the_document_needs_one() {
    let mut opaque = PsdBuilder::new(1, 1);
    opaque.add_layer(LayerBuilder::new("Opaque").rgba(1, 1, RED.to_vec()));
    let opaque = opaque.to_bytes().unwrap();

    let mut transparent = PsdBuilder::new(1, 1);
    transparent.add_layer(LayerBuilder::new("Transparent").rgba(1, 1, vec![255, 0, 0, 128]));
    let transparent = transparent.to_bytes().unwrap();

    // Bytes 12 and 13 of the file header are the channel count.
    assert_eq!(&opaque[12..14], &[0, 3]);
    assert_eq!(&transparent[12..14], &[0, 4]);

    assert_eq!(Psd::from_bytes(&opaque).unwrap().rgba()[3], 255);
    assert_eq!(Psd::from_bytes(&transparent).unwrap().rgba()[3], 128);
}

/// Layer names round trip through the unicode name block, so they aren't
/// limited to ASCII.
#[test]
fn round_trips_unicode_layer_names() {
    let names = ["Кириллица", "中文图层", "emoji 🎨", "with spaces"];

    let mut builder = PsdBuilder::new(1, 1);
    for name in names {
        builder.add_layer(LayerBuilder::new(name).rgba(1, 1, GREEN.to_vec()));
    }
    builder.add_group(GroupBuilder::new("группа"));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    for name in names {
        assert!(
            psd.layer_by_name(name).is_some(),
            "could not find a layer named {}",
            name
        );
    }

    assert_eq!(psd.groups().get(&1).unwrap().name(), "группа");
}

/// A name that is too long for the record's Pascal string still round trips,
/// because the unicode name block holds the whole thing.
#[test]
fn round_trips_a_very_long_layer_name() {
    let name = "a".repeat(400);

    let mut builder = PsdBuilder::new(1, 1);
    builder.add_layer(LayerBuilder::new(name.clone()).rgba(1, 1, GREEN.to_vec()));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers()[0].name(), name);
}

/// Layers that hang off of the edge of the canvas are allowed.
#[test]
fn writes_a_layer_that_hangs_off_of_the_canvas() {
    let mut builder = PsdBuilder::new(2, 2);
    builder.add_layer(
        LayerBuilder::new("Overhang")
            .rgba(4, 4, solid(4, 4, GREEN))
            .at(-1, -1),
    );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let layer = psd.layer_by_name("Overhang").unwrap();
    assert_eq!(layer.layer_left(), -1);
    assert_eq!(layer.layer_top(), -1);

    // The parts of the layer that are on the canvas still show up.
    assert_eq!(psd.rgba(), solid(2, 2, GREEN));
}

/// `flatten_layers_rgba` works on a PSD that we wrote.
#[test]
fn flattens_layers_of_a_written_psd() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(LayerBuilder::new("Bottom").rgba(1, 1, RED.to_vec()))
        .add_layer(LayerBuilder::new("_ignored").rgba(1, 1, BLUE.to_vec()));

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let flattened = psd
        .flatten_layers_rgba(&|(_idx, layer)| !layer.name().starts_with('_'))
        .unwrap();

    assert_eq!(flattened, RED.to_vec());
}

/// A layer can be clipped to the layer below it.
#[test]
fn round_trips_a_clipped_layer() {
    let mut builder = PsdBuilder::new(1, 1);
    builder
        .add_layer(LayerBuilder::new("Base").rgba(1, 1, RED.to_vec()))
        .add_layer(
            LayerBuilder::new("Clipped")
                .rgba(1, 1, GREEN.to_vec())
                .clipped_to_layer_below(true),
        );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    // `is_clipping_mask` is true for the layer that others clip *to*, so it
    // reads back as the opposite of the flag that we set.
    assert_eq!(psd.layer_by_name("Base").unwrap().is_clipping_mask(), true);
    assert_eq!(
        psd.layer_by_name("Clipped").unwrap().is_clipping_mask(),
        false
    );
}

/// Every blend mode writes a key that we can read back.
#[test]
fn round_trips_every_blend_mode() {
    let blend_modes = [
        BlendMode::PassThrough,
        BlendMode::Normal,
        BlendMode::Dissolve,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::DarkerColor,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::LinearDodge,
        BlendMode::LighterColor,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    let mut builder = PsdBuilder::new(1, 1);
    for (idx, blend_mode) in blend_modes.iter().enumerate() {
        builder.add_layer(
            LayerBuilder::new(format!("{}", idx))
                .rgba(1, 1, GREEN.to_vec())
                .blend_mode(*blend_mode),
        );
    }

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    for (idx, blend_mode) in blend_modes.iter().enumerate() {
        assert_eq!(
            psd.layer_by_name(&format!("{}", idx)).unwrap().blend_mode(),
            *blend_mode
        );
    }
}

/// A larger document with many layers, to exercise the length markers that hold
/// the layer info section together.
#[test]
fn round_trips_many_layers() {
    let mut builder = PsdBuilder::new(64, 64);

    for idx in 0..50 {
        let shade = (idx * 5) as u8;
        builder.add_layer(
            LayerBuilder::new(format!("Layer {}", idx))
                .rgba(5, 5, solid(5, 5, [shade, shade, shade, 255]))
                .at(idx as i32, idx as i32),
        );
    }

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    assert_eq!(psd.layers().len(), 50);
    assert_eq!(psd.layer_by_name("Layer 49").unwrap().layer_left(), 49);
}

#[test]
fn rejects_a_canvas_that_is_too_big() {
    let error = PsdBuilder::new(30_001, 1).to_bytes().unwrap_err();

    assert_eq!(
        error,
        PsdWriteError::InvalidCanvasSize {
            width: 30_001,
            height: 1
        }
    );
}

#[test]
fn rejects_a_zero_sized_canvas() {
    let error = PsdBuilder::new(0, 5).to_bytes().unwrap_err();

    assert_eq!(
        error,
        PsdWriteError::InvalidCanvasSize {
            width: 0,
            height: 5
        }
    );
}

#[test]
fn rejects_pixels_that_do_not_match_the_layer_size() {
    let mut builder = PsdBuilder::new(1, 1);
    builder.add_layer(LayerBuilder::new("Wrong").rgba(2, 2, vec![0; 4]));

    let error = builder.to_bytes().unwrap_err();

    assert_eq!(
        error,
        PsdWriteError::LayerPixelCountMismatch {
            layer_name: "Wrong".to_string(),
            width: 2,
            height: 2,
            expected: 16,
            actual: 4,
        }
    );
}

#[test]
fn rejects_a_flattened_image_that_does_not_match_the_canvas() {
    let mut builder = PsdBuilder::new(2, 2);
    builder.flattened_image(vec![0; 4]);

    let error = builder.to_bytes().unwrap_err();

    assert_eq!(
        error,
        PsdWriteError::FlattenedImagePixelCountMismatch {
            expected: 16,
            actual: 4,
        }
    );
}

#[test]
fn rejects_unsupported_compression() {
    let mut builder = PsdBuilder::new(1, 1);
    builder.compression(PsdChannelCompression::ZipWithPrediction);

    let error = builder.to_bytes().unwrap_err();

    assert_eq!(
        error,
        PsdWriteError::UnsupportedCompression {
            compression: PsdChannelCompression::ZipWithPrediction
        }
    );
}

/// A layer inside of a nested group can be found by name, and its pixels are
/// the ones that we gave it.
#[test]
fn nested_layer_keeps_its_pixels() {
    let mut builder = PsdBuilder::new(4, 4);
    builder.add_group(
        GroupBuilder::new("outer").add_group(
            GroupBuilder::new("inner").add_layer(
                LayerBuilder::new("Nested")
                    .rgba(2, 2, solid(2, 2, BLUE))
                    .at(1, 1),
            ),
        ),
    );

    let bytes = builder.to_bytes().unwrap();
    let psd = Psd::from_bytes(&bytes).unwrap();

    let nested = psd.layer_by_name("Nested").unwrap();

    let mut expected = vec![0; 4 * 4 * 4];
    for row in 1..3 {
        for column in 1..3 {
            let idx = (row * 4 + column) * 4;
            expected[idx..idx + 4].copy_from_slice(&BLUE);
        }
    }

    assert_eq!(nested.rgba(), expected);
}
