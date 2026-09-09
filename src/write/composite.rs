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

    for row in 0..layer.height as i32 {
        let canvas_row = layer.top + row;
        if canvas_row < 0 || canvas_row >= height {
            continue;
        }

        for column in 0..layer.width as i32 {
            let canvas_column = layer.left + column;
            if canvas_column < 0 || canvas_column >= width {
                continue;
            }

            let source = (row as usize * layer.width as usize + column as usize) * 4;
            let source_alpha = multiply(layer.rgba[source + 3], opacity);
            if source_alpha == 0 {
                continue;
            }

            let destination = (canvas_row as usize * width as usize + canvas_column as usize) * 4;

            source_over(
                &[
                    layer.rgba[source],
                    layer.rgba[source + 1],
                    layer.rgba[source + 2],
                    source_alpha,
                ],
                &mut canvas[destination..destination + 4],
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

    #[test]
    fn multiplies_like_a_fraction() {
        assert_eq!(multiply(255, 255), 255);
        assert_eq!(multiply(255, 0), 0);
        assert_eq!(multiply(128, 255), 128);
        assert_eq!(multiply(128, 128), 64);
    }
}
