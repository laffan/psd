# Creating PSDs

Along with parsing PSD files the crate can write them. The entry point is
`PsdBuilder`, which takes a canvas size and a stack of layers and groups and
hands back the bytes of a PSD file.

```rust
use psd::{LayerBuilder, PsdBuilder};

let mut psd = PsdBuilder::new(2, 2);
psd.add_layer(LayerBuilder::new("Background").rgba(2, 2, vec![255, 0, 0, 255].repeat(4)));

let bytes: Vec<u8> = psd.to_bytes().unwrap();
```

## Layer order

Layers and groups are added from the bottom of the layer stack upwards, so the
first thing you add is the bottom-most thing in the document. This is the order
that a PSD file itself stores its layer records in.

Note that reading is the other way around - `Psd::layers()` hands back layers
in the order that Photoshop's layers panel shows them, top first. So a document
you write bottom-up reads back top-down.

```rust
use psd::{LayerBuilder, Psd, PsdBuilder};

let mut psd = PsdBuilder::new(1, 1);
psd.add_layer(LayerBuilder::new("Bottom").rgba(1, 1, vec![0, 255, 0, 255]))
   .add_layer(LayerBuilder::new("Top").rgba(1, 1, vec![0, 0, 255, 255]));

let psd = Psd::from_bytes(&psd.to_bytes().unwrap()).unwrap();

assert_eq!(psd.layers()[0].name(), "Top");
assert_eq!(psd.layers()[1].name(), "Bottom");
```

## Nesting

A `GroupBuilder` holds layers and other groups, so groups are how you nest.
There is no depth limit.

```rust
use psd::{GroupBuilder, LayerBuilder, PsdBuilder};

let inner = GroupBuilder::new("Inner")
    .add_layer(LayerBuilder::new("Deep").rgba(1, 1, vec![0, 255, 0, 255]));

let outer = GroupBuilder::new("Outer")
    .add_layer(LayerBuilder::new("Shallow").rgba(1, 1, vec![255, 0, 0, 255]))
    .add_group(inner);

let mut psd = PsdBuilder::new(1, 1);
psd.add_group(outer);
```

Under the hood a group is written as two layer records - a hidden bounding
section record that closes the folder, and a record that opens it and carries
the group's name, opacity, visibility and blend mode. Everything in between
those two records belongs to the group. That's why every group costs two of the
32,767 layer records that a PSD can hold.

## Pixels

`LayerBuilder::rgba` takes the layer's dimensions and `width * height * 4`
bytes of `[R, G, B, A, R, G, B, A, ...]`, the same layout that
`PsdLayer::rgba()` returns.

A layer doesn't have to cover the whole canvas. `LayerBuilder::at` positions
its top left corner, and layers are allowed to hang off of the edge of the
canvas - Photoshop keeps the pixels that don't fit.

A layer with no pixels at all is valid too. `LayerBuilder::new("name")` on its
own gives you an empty layer.

## Compression

Channel data is RLE (PackBits) compressed by default, which is what Photoshop
writes. `PsdBuilder::compression` switches to `PsdChannelCompression::RawData`,
which writes faster and produces bigger files. ZIP compression is not
supported.

## The flattened image

The last section of a PSD holds a flattened copy of the whole document. It is
what image viewers show and what `Psd::rgba()` reads.

We composite the layer stack for you using ordinary source-over alpha
compositing, honouring each layer's and group's opacity and visibility. Blend
modes are *not* applied while flattening, though they are written into the
layer records - so Photoshop re-composites the document correctly as soon as it
opens the file. If you need exact control, hand us your own flattened image
with `PsdBuilder::flattened_image`.

When a document has transparency in it we write its flattened image the way
Photoshop does: an alpha channel, plus colour channels that have already been
composited over white. A fully transparent pixel is stored as white rather than
black, so readers that undo the matte recover the original colour.

## What isn't written

- Layer masks and vector masks
- Adjustment, text and shape layers
- Colour modes other than RGB, and depths other than 8 bits
- Image resources and colour mode data, which are written as empty sections

Please open an issue if you need any of these.
