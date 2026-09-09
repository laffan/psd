//! Create a PSD file with nested groups.
//!
//! ```sh
//! cargo run --example create_psd -- /tmp/created.psd
//! ```

use std::env;
use std::fs;

use psd::{BlendMode, GroupBuilder, LayerBuilder, PsdBuilder};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

fn main() {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "created.psd".to_string());

    let mut psd = PsdBuilder::new(WIDTH, HEIGHT);

    // The bottom of the stack is a checkerboard that fills the canvas.
    psd.add_layer(LayerBuilder::new("Background").rgba(WIDTH, HEIGHT, checkerboard()));

    // A group holding two squares, one of which is in a group of its own.
    let shapes = GroupBuilder::new("Shapes")
        .add_layer(
            LayerBuilder::new("Red Square")
                .rgba(24, 24, solid(24, 24, [220, 60, 60, 255]))
                .at(8, 8),
        )
        .add_group(
            GroupBuilder::new("Highlights").add_layer(
                LayerBuilder::new("Blue Square")
                    .rgba(24, 24, solid(24, 24, [60, 90, 220, 255]))
                    .at(28, 28)
                    .opacity(160)
                    .blend_mode(BlendMode::Multiply),
            ),
        );

    psd.add_group(shapes);

    // A hidden layer, the way you might mark up a source file for a tool that
    // skips layers by name.
    psd.add_layer(
        LayerBuilder::new("_guides")
            .rgba(WIDTH, 1, solid(WIDTH, 1, [255, 255, 0, 255]))
            .at(0, 32)
            .visible(false),
    );

    let bytes = psd.to_bytes().expect("could not create the PSD");

    fs::write(&path, &bytes).expect("could not write the PSD");

    println!("Wrote {} bytes to {}", bytes.len(), path);
}

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    rgba.iter()
        .copied()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}

fn checkerboard() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);

    for row in 0..HEIGHT {
        for column in 0..WIDTH {
            let shade = if (row / 8 + column / 8) % 2 == 0 {
                235
            } else {
                205
            };
            pixels.extend_from_slice(&[shade, shade, shade, 255]);
        }
    }

    pixels
}
