psd
===

> A Rust API for reading and writing Adobe Photoshop (PSD) files.

This is a fork of [chinedufn/psd][upstream], maintained in support of a Rust port of
[psd-to-json](https://github.com/laffan/psd-to-json). It keeps the upstream parsing API intact and
adds mask access, per-layer compositing and the ability to **create** PSD files. See
[UPDATES.md](UPDATES.md) for a change-by-change account of what this fork adds, and
[CHANGELOG.md](CHANGELOG.md) for the upstream-style changelog.

The fork branched from upstream at [`28357a2`][fork-point], one commit past the `0.3.5` release.

[upstream]: https://github.com/chinedufn/psd
[fork-point]: https://github.com/chinedufn/psd/commit/28357a2

## Installing

This fork isn't published to crates.io, so depend on it by git:

```toml
[dependencies]
psd = { git = "https://github.com/laffan/psd" }
```

The upstream crate is on [crates.io](https://crates.io/crates/psd) and
[docs.rs](https://docs.rs/psd), but those don't include anything on this page marked as a fork
addition. To read this fork's API docs, run `cargo doc --open`.

## Usage

### Reading a PSD

```rust
use psd::{BlendMode, ColorMode, Psd, PsdChannelCompression, PsdChannelKind};

fn main () {
    // .. Get a byte slice of PSD file data somehow ..
    let psd = include_bytes!("./my-psd-file.psd");

    let psd = Psd::from_bytes(psd).unwrap();

    assert_eq!(psd.color_mode(), ColorMode::Rgb);

    // For this PSD the final combined image is RleCompressed
    assert_eq!(psd.compression(), &PsdChannelCompression::RleCompressed);

    assert_eq!(psd.width(), 500);
    assert_eq!(psd.height(), 500);

    // Get the combined final image for the PSD.
    let final_image: Vec<u8> = psd.rgba();

    // Layers come back in the order that Photoshop's layers panel shows them,
    // top first.
    for layer in psd.layers().iter() {
        let name = layer.name();

        // Raw decoded channels (no opacity or mask applied).
        let raw_pixels: Vec<u8> = layer.rgba();

        // Composited pixels with opacity and raster mask baked in —
        // closer to what Photoshop displays.
        let sprite_pixels: Vec<u8> = layer.composite_rgba();
    }

    let green_layer = psd.layer_by_name("Green Layer").unwrap();

    // In this layer the red channel is uncompressed
    assert_eq!(green_layer.compression(PsdChannelKind::Red).unwrap(), PsdChannelCompression::RawData);

    // In this layer the green channel is RLE compressed
    assert_eq!(green_layer.compression(PsdChannelKind::Green).unwrap(), PsdChannelCompression::RleCompressed);

    // Combine the PSD layers top to bottom, ignoring any layers that begin with an `_`
    let pixels: Vec<u8> = psd.flatten_layers_rgba(&|(_idx, layer)| {
        !layer.name().starts_with("_")
    }).unwrap();

    // Get computed bounds for a group (union of descendant layers).
    for (&id, group) in psd.groups() {
        if let Some((top, left, bottom, right)) = psd.group_bounds(id) {
            let w = (right - left) + 1;
            let h = (bottom - top) + 1;
            println!("group '{}' covers {}x{} at ({}, {})", group.name(), w, h, left, top);
        }
    }
}
```

### Creating a PSD

`PsdBuilder` writes a PSD from scratch, with layers and nested groups.

```rust
use psd::{BlendMode, GroupBuilder, LayerBuilder, PsdBuilder};

fn main () {
    let mut psd = PsdBuilder::new(64, 64);

    // Layers are added from the bottom of the layer stack upwards.
    psd.add_layer(
        LayerBuilder::new("Background")
            // [R, G, B, A, R, G, B, A, ...], width * height * 4 bytes long.
            .rgba(64, 64, vec![255, 255, 255, 255].repeat(64 * 64))
    );

    // Groups hold layers and other groups, so they're how you nest.
    let shapes = GroupBuilder::new("Shapes")
        .add_layer(
            LayerBuilder::new("Red Square")
                .rgba(24, 24, vec![220, 60, 60, 255].repeat(24 * 24))
                .at(8, 8)
        )
        .add_group(
            GroupBuilder::new("Highlights").add_layer(
                LayerBuilder::new("Blue Square")
                    .rgba(24, 24, vec![60, 90, 220, 255].repeat(24 * 24))
                    .at(28, 28)
                    .opacity(160)
                    .blend_mode(BlendMode::Multiply)
            )
        );

    psd.add_group(shapes);

    let bytes: Vec<u8> = psd.to_bytes().unwrap();

    std::fs::write("./my-psd-file.psd", bytes).unwrap();
}
```

For a runnable version:

```sh
cargo run --example create_psd -- /tmp/created.psd
```

## What's supported

### Reading

| | |
|---|---|
| Document | Width, height, bit depth, colour mode, channel count |
| Layers | Name (including the unicode `luni` name), position, opacity, visibility, blend mode, clipping flag |
| Groups | Arbitrary nesting, parent/child relationships, computed bounds |
| Masks | Raster mask bounding box, flags and pixels; vector mask Bézier paths |
| Channels | Raw and RLE (PackBits) compressed |
| Image resources | Slices (v6, v7 and v8) |
| Rendering | Per-layer RGBA, per-layer composite with opacity and mask applied, whole-document flattening with blend modes |

### Writing

| | |
|---|---|
| Document | 8 bit RGB at any size the format allows |
| Layers | Name, position, opacity, visibility, blend mode, clipping flag, pixels |
| Groups | Arbitrary nesting, name, opacity, visibility, blend mode, collapsed state |
| Channels | RLE (PackBits) by default, raw on request |
| Flattened image | Composited from the layer stack, or supplied by the caller |

### Not supported

The Photoshop specification is large and this crate covers the parts that its users have needed.
[Please open an issue][issues] if something you need is missing.

- **PSB** (large document format). Adding it should be straightforward.
- **ZIP compressed channels**, with and without prediction.
- **Colour modes other than RGB** — CMYK, Indexed, Grayscale, Lab and Duotone may parse but are
  untested and can produce wrong pixels. Only 8 bit depth is well tested; 16 bit is partially
  handled and 32 bit is not.
- **12 of the 28 blend modes** when flattening a document — pass through, dissolve, darker colour,
  lighter colour, vivid light, linear light, pin light, hard mix, hue, saturation, colour and
  luminosity will panic in `flatten_layers_rgba`. Every blend mode is *read and written* correctly,
  so `layer.blend_mode()` is always right; only the built-in renderer is incomplete.
- **Vector mask clipping** when compositing. The path data is exposed so a caller can rasterize it.
- **Writing masks, adjustment layers, text layers and image resources.** A parse, modify, write
  round trip therefore loses masks.

[issues]: https://github.com/laffan/psd/issues

## Documentation

- **API docs** — `cargo doc --open`
- **The Psd Book** — `cd book && mdbook build`, or read the sources in [book/src](book/src). The
  [hosted copy](https://chinedufn.github.io/psd) is built from upstream and doesn't include this
  fork's chapters.
- **[UPDATES.md](UPDATES.md)** — everything this fork changed, and why

## Examples

### Creating a PSD

[examples/create_psd.rs](examples/create_psd.rs) builds a document with a background, a group, a
nested group and a hidden layer, then writes it to disk.

```sh
cargo run --example create_psd -- /tmp/created.psd
```

### Drag and drop browser demo

The crate compiles to WebAssembly. The demo visualizes a PSD in the browser, lets you toggle layers
on and off, and accepts a new PSD by drag and drop.

[![Demo screenshot](./examples/drag-drop-browser/demo-screenshot.png)](https://chinedufn.github.io/psd/drag-drop-demo/)

[The live demo](https://chinedufn.github.io/psd/drag-drop-demo/) is built from upstream. See
[examples/drag-drop-browser](examples/drag-drop-browser) for instructions on running it against this
fork locally.

## Tests

```sh
cargo test --all
```

Tests parse the PSD files in [tests/fixtures](tests/fixtures), each of which is described in
[tests/fixtures/README.md](tests/fixtures/README.md). [tests/write_psd.rs](tests/write_psd.rs)
round trips everything the writer produces back through the parser.

[tests/psd_to_json_integration.rs](tests/psd_to_json_integration.rs) exercises the features that a
Rust port of psd-to-json depends on and can be pointed at your own files:

```sh
PSD_TO_JSON_INPUT=/path/to/psds cargo test --test psd_to_json_integration -- --nocapture
```

## Background

From the original author, on why the crate exists:

> I'm working on a game and part of my asset compilation process was a script that did the
> following:
>
> 1. Iterate over all PSD files
> 2. Export every PSD into a PNG, ignoring any layers that begin with an `_`
> 3. Combine PNGs into a texture atlas
>
> For a couple of years I was using `imagemagick` to power step 2, but after getting a new laptop
> and upgrading `imagemagick` versions it stopped working.
>
> After a bit of Googling I couldn't land on a solution for my problem so I decided to make this
> crate.
>
> My approach was to support as much of the PSD spec as I needed, so there might be bits of
> information that you'd like to make use of that aren't currently supported.

The fork exists for a similar reason: [psd-to-json](https://github.com/laffan/psd-to-json) leans on
Python's [psd-tools](https://github.com/psd-tools/psd-tools), and porting it to Rust needed mask
data, per-layer compositing and PSD creation that the crate didn't have yet.

## Acknowledgements

`psd` was created by [Chinedu Francis Nwafili](https://github.com/chinedufn) and its
[contributors](https://github.com/chinedufn/psd/graphs/contributors). Everything here builds on
their work — the parser, the section-by-section architecture, the test fixtures and the book are
all theirs. Please send bug reports that aren't specific to this fork
[upstream](https://github.com/chinedufn/psd/issues).

[psd-tools](https://github.com/psd-tools/psd-tools) was used as an independent implementation to
check that the files this crate writes are valid.

## See Also

- [PSD specification](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/) — the basis of
  our API
- [psd-tools](https://github.com/psd-tools/psd-tools) — a mature Python implementation
- [ag-psd](https://github.com/Agamnentzar/ag-psd) — a JavaScript reader and writer

## License

Dual licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
