//! Flatten a stack of layers into the image that goes in a PSD's image data
//! section.
//!
//! This is deliberately simple - ordinary source-over alpha compositing with
//! each layer's and group's opacity applied. Blend modes are written into the
//! layer records but are not applied here, so Photoshop's own composite of the
//! same document can differ until it re-saves the file.

use crate::write::{Child, GroupBuilder, LayerBuilder};

/// Composite a layer stack into `width * height * 4` RGBA bytes.
pub(super) fn flatten(width: u32, height: u32, children: &[Child]) -> Vec<u8> {
    let mut canvas = vec![0; width as usize * height as usize * 4];

    draw_children(children, &mut canvas, width as i32, height as i32, 255);

    canvas
}

/// Draw a stack of layers and groups onto the canvas, bottom-most first.
fn draw_children(children: &[Child], canvas: &mut [u8], width: i32, height: i32, opacity: u8) {
    for child in children {
        match child {
            Child::Layer(layer) => draw_layer(layer, canvas, width, height, opacity),
            Child::Group(group) => draw_group(group, canvas, width, height, opacity),
        }
    }
}

/// A group's opacity multiplies into its children's, which is how a
/// pass through group behaves.
fn draw_group(group: &GroupBuilder, canvas: &mut [u8], width: i32, height: i32, opacity: u8) {
    if !group.visible {
        return;
    }

    let opacity = multiply(group.opacity, opacity);
    if opacity == 0 {
        return;
    }

    draw_children(&group.children, canvas, width, height, opacity);
}

fn draw_layer(layer: &LayerBuilder, canvas: &mut [u8], width: i32, height: i32, opacity: u8) {
    if !layer.visible {
        return;
    }

    let opacity = multiply(layer.opacity, opacity);
    if opacity == 0 {
        return;
    }

    // The columns of the layer that land on the canvas, worked out once
    // rather than tested for every pixel.
    let first_column = (-layer.left).max(0);
    let last_column = (width - layer.left).min(layer.width as i32);
    if first_column >= last_column {
        return;
    }
    let run = (last_column - first_column) as usize * 4;

    for row in 0..layer.height as i32 {
        let canvas_row = layer.top + row;
        if canvas_row < 0 || canvas_row >= height {
            continue;
        }

        let source_start = (row as usize * layer.width as usize + first_column as usize) * 4;
        let destination_start =
            (canvas_row as usize * width as usize + (layer.left + first_column) as usize) * 4;
        let source = &layer.rgba[source_start..source_start + run];
        let destination = &mut canvas[destination_start..destination_start + run];

        for (source, destination) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
            let source_alpha = multiply(source[3], opacity);
            if source_alpha == 0 {
                continue;
            }

            // Two cases `source_over` answers with the source pixel itself:
            // nothing underneath, or nothing showing through. Most of a
            // sketch is the first and most of a painting the second.
            if destination[3] == 0 || source_alpha == 255 {
                destination[..3].copy_from_slice(&source[..3]);
                destination[3] = source_alpha;
                continue;
            }

            source_over(
                &[source[0], source[1], source[2], source_alpha],
                destination,
            );
        }
    }
}

/// Composite a straight (not premultiplied) RGBA pixel onto the pixel below it.
///
/// `αo = αs + αb x (1 - αs)`
/// `Co = (Cs x αs + Cb x αb x (1 - αs)) / αo`
fn source_over(source: &[u8; 4], destination: &mut [u8]) {
    let source_alpha = source[3] as u32;
    let destination_alpha = destination[3] as u32;

    // The output alpha, scaled up by 255 so that we can stay in integers.
    let out_alpha = source_alpha * 255 + destination_alpha * (255 - source_alpha);
    if out_alpha == 0 {
        return;
    }

    for channel in 0..3 {
        let source_contribution = source[channel] as u32 * source_alpha * 255;
        let destination_contribution =
            destination[channel] as u32 * destination_alpha * (255 - source_alpha);

        destination[channel] = ((source_contribution + destination_contribution) / out_alpha) as u8;
    }

    destination[3] = ((out_alpha + 127) / 255) as u8;
}

/// Composite straight alpha RGBA pixels over a white background, which is how
/// Photoshop stores a transparent document's flattened image.
///
/// The alpha channel is left alone, so a reader can undo this to recover the
/// original colour. A fully transparent pixel becomes white.
pub(super) fn matte_over_white(rgba: &[u8]) -> Vec<u8> {
    let mut matted = rgba.to_vec();

    for pixel in matted.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha == 255 {
            continue;
        }

        for channel in 0..3 {
            let color = pixel[channel] as u32 * alpha;
            let white = 255 * (255 - alpha);

            pixel[channel] = ((color + white) / 255) as u8;
        }
    }

    matted
}

/// Multiply two 0-255 values as though they were 0.0-1.0.
fn multiply(left: u8, right: u8) -> u8 {
    ((left as u16 * right as u16 + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_source_replaces_the_destination() {
        let mut destination = [1, 2, 3, 255];
        source_over(&[10, 20, 30, 255], &mut destination);

        assert_eq!(destination, [10, 20, 30, 255]);
    }

    #[test]
    fn drawing_onto_nothing_keeps_the_source() {
        let mut destination = [0, 0, 0, 0];
        source_over(&[10, 20, 30, 128], &mut destination);

        assert_eq!(destination, [10, 20, 30, 128]);
    }

    #[test]
    fn half_transparent_white_over_black_is_grey() {
        let mut destination = [0, 0, 0, 255];
        source_over(&[255, 255, 255, 128], &mut destination);

        assert_eq!(destination[3], 255);
        assert!((127..=129).contains(&destination[0]));
    }

    /// Photoshop writes a fully transparent pixel as white.
    #[test]
    fn transparent_pixels_matte_to_white() {
        assert_eq!(matte_over_white(&[10, 20, 30, 0]), vec![255, 255, 255, 0]);
    }

    /// Opaque pixels come out the way that they went in.
    #[test]
    fn opaque_pixels_are_left_alone() {
        assert_eq!(matte_over_white(&[10, 20, 30, 255]), vec![10, 20, 30, 255]);
    }

    /// These are the bytes that Photoshop itself wrote into
    /// `tests/fixtures/blending/blue-red-1x1-normal.psd`, whose true composite
    /// is `[85, 0, 170, 192]`.
    #[test]
    fn matches_the_bytes_that_photoshop_writes() {
        assert_eq!(
            matte_over_white(&[85, 0, 170, 192]),
            vec![127, 63, 191, 192]
        );
    }

    /// The two cases `draw_layer` answers without `source_over` are ones
    /// `source_over` would answer the same way, for every input.
    #[test]
    fn the_shortcuts_are_what_source_over_says() {
        for alpha in 1..=255u8 {
            for colour in [0u8, 1, 77, 128, 254, 255] {
                let source = [colour, 255 - colour, colour / 3, alpha];
                for below in [[0u8, 0, 0, 0], [9, 200, 31, 0]] {
                    let mut destination = below;
                    source_over(&source, &mut destination);
                    assert_eq!(destination, source);
                }
            }
        }
        for below in [[9u8, 200, 31, 1], [0, 0, 0, 128], [255, 255, 255, 255]] {
            for colour in [0u8, 77, 255] {
                let source = [colour, colour, 255 - colour, 255];
                let mut destination = below;
                source_over(&source, &mut destination);
                assert_eq!(destination, source);
            }
        }
    }

    #[test]
    fn multiplies_like_a_fraction() {
        assert_eq!(multiply(255, 255), 255);
        assert_eq!(multiply(255, 0), 0);
        assert_eq!(multiply(128, 255), 128);
        assert_eq!(multiply(128, 128), 64);
    }
}
