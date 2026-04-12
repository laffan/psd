# Fork Updates

Changes made to this fork of [chinedufn/psd](https://github.com/chinedufn/psd)
in support of building a Rust port of
[psd-to-json](https://github.com/laffan/psd-to-json).

All changes live on the `claude/prepare-psd-rust-conversion-aOGdA` branch.

---

## New API

### `PsdLayer::composite_rgba()`

```rust
let composited: Vec<u8> = layer.composite_rgba();
```

Returns a canvas-sized RGBA buffer (`width * height * 4` bytes) with the
layer's **opacity** and **raster mask** baked into the alpha channel. This is
the Rust equivalent of psd-tools' `layer.composite()` and should be used
anywhere a layer needs to be exported as a visible sprite.

Compared to the existing `PsdLayer::rgba()` (which returns the raw decoded
channel data with no post-processing), `composite_rgba()` applies two steps:

1. **Opacity** -- multiplies every pixel's alpha by `layer.opacity() / 255`.
2. **Raster mask** -- for each pixel, looks up the mask value (using the mask's
   own bounding box and `default_color` for pixels outside it), handles
   `mask.invert`, and multiplies the result into alpha.

Vector mask clipping is not yet applied. Shape layers that rely solely on a
vector mask will still show their full bounding-box content. This is a known
limitation documented in the method's docstring.

**Source:** `src/sections/layer_and_mask_information_section/layer.rs`

---

### `Psd::group_bounds(group_id)`

```rust
if let Some((top, left, bottom, right)) = psd.group_bounds(group.id()) {
    let width  = (right - left) + 1;
    let height = (bottom - top) + 1;
}
```

Computes the bounding box of a group as the union of all its descendant
layers' bounds. Returns `Option<(i32, i32, i32, i32)>` — `None` if the group
ID is unknown or the group contains no non-empty layers.

PSD group divider records typically carry zeroed coordinates, so the existing
accessors on `PsdGroup` (`layer_top()`, `layer_left()`, etc.) are unreliable
for real-world files. This method replaces them for any code that needs to know
how large a group is (e.g. sizing a sprite output buffer for a group composite).

**Source:** `src/lib.rs`

---

### `PsdGroup::contained_layers()`

```rust
for idx in group.contained_layers() {
    let layer = psd.layer_by_idx(idx);
    // ...
}
```

Public accessor for the flat-index range of layers nested under the group (at
any depth -- includes layers in subgroups). Filter by `layer.parent_id()` if
you only need direct children.

**Source:** `src/sections/layer_and_mask_information_section/layer.rs`

---

### `BlendMode` re-export

```rust
use psd::BlendMode;

match layer.blend_mode() {
    BlendMode::Normal => { /* ... */ }
    BlendMode::Multiply => { /* ... */ }
    // ... all 28 variants
    _ => {}
}
```

`BlendMode` is now re-exported at the crate root (`psd::BlendMode`), so
downstream code can name the enum directly in match arms without reaching into
the private `sections` module path.

**Source:** `src/lib.rs`

---

## Integration Test

A new integration test at `tests/psd_to_json_integration.rs` exercises every
PSD feature that the Python psd-to-json project depends on.

### Running the built-in fixtures

```
cargo test --test psd_to_json_integration -- --nocapture
```

This processes 6 bundled fixture PSDs and writes outputs to
`target/psd-to-json-out/<fixture>/`:

| Fixture | What it exercises |
|---|---|
| `two-layers-red-green-1x1` | Basic multi-layer parsing |
| `rle-3-layer-8x8` | RLE decompression + PNG encoding |
| `one-group-with-two-subgroups` | Deeply nested groups, sibling ordering |
| `visibility` | Visible/invisible layer flags |
| `16x16-rle-partially-opaque` | Partial opacity + alpha channels |
| `vector-mask-and-layer-mask` | Raster masks, vector masks, composite_rgba |

### Running against your own PSDs

```
PSD_TO_JSON_INPUT=/path/to/file.psd \
  cargo test --test psd_to_json_integration psd_to_json_user_input -- --nocapture
```

The `PSD_TO_JSON_INPUT` env var accepts a single `.psd` file or a directory
containing `.psd` files. Outputs land in
`target/psd-to-json-out/user-input/<file_stem>/`.

### Output structure

For each PSD, the test writes:

```
<name>/
  canvas.png                  # full flattened composite (flatten_layers_rgba)
  data.json                   # nested layer/group tree
  layers/
    00_LayerName.png           # per-layer composite (composite_rgba)
    01_AnotherLayer.png
  masks/
    00_LayerName_mask.png      # grayscale raster mask (mask_pixels)
```

### What the test asserts

- Document dimensions, layer counts, group counts, parent-group relationships
- `composite_rgba()` equals `rgba()` when opacity is 255 and no mask is present
- `composite_rgba()` alpha <= `rgba()` alpha when opacity < 255
- Pixels outside a mask bbox have alpha == 0 in the composite (for masks with
  `default_color == 0`)
- `group_bounds()` fully encloses every descendant layer
- `BlendMode` can be named and matched exhaustively via `psd::BlendMode`
- `contained_layers()` range includes every layer whose parent-ID chain reaches
  the group
- Root-level siblings appear in Photoshop document order (groups and layers
  interleaved correctly)
- JSON round-trips through serde_json

---

## `data.json` schema

The JSON produced by the test mirrors the structure that psd-to-json emits,
making it a reference target for a Rust port. Key fields:

**Document level:**

```json
{
  "width": 100,
  "height": 100,
  "color_mode": "Rgb",
  "depth": "Eight",
  "layers": [ ... ]
}
```

**Layer:**

```json
{
  "kind": "layer",
  "index": 0,
  "depth": 2,
  "name": "Sprite Layer",
  "visible": true,
  "opacity": 200,
  "alpha": 0.784,
  "blend_mode": "Normal",
  "clipping_mask": false,
  "x": 10, "y": 20,
  "width": 50, "height": 30,
  "bbox": { "left": 10, "top": 20, "right": 59, "bottom": 49 },
  "mask": {
    "left": 15, "top": 25, "right": 55, "bottom": 45,
    "width": 40, "height": 20,
    "default_color": 0,
    "disabled": false,
    "invert": false,
    "relative_to_layer": false
  },
  "vector_mask": {
    "disabled": false,
    "invert": false,
    "not_linked": false,
    "paths": [{
      "closed": true,
      "knots": [{
        "anchor": [0.25, 0.25],
        "preceding": [0.25, 0.25],
        "following": [0.25, 0.25],
        "linked": true
      }]
    }]
  }
}
```

`mask` and `vector_mask` only appear when the layer has them.

**Group:**

```json
{
  "kind": "group",
  "id": 1,
  "depth": 0,
  "name": "My Group",
  "visible": true,
  "opacity": 255,
  "alpha": 1.0,
  "blend_mode": "PassThrough",
  "x": 10, "y": 20,
  "width": 80, "height": 60,
  "computed_bounds": { "top": 20, "left": 10, "bottom": 79, "right": 89 },
  "children": [ ... ]
}
```

Group `x`, `y`, `width`, and `height` are derived from `computed_bounds` (the
union of descendant layers), not from the unreliable group divider record.

---

## Dev dependencies added

- `png = "0.17"` -- PNG encoding for test outputs
- `serde_json = "1"` -- JSON serialization for `data.json`

These are dev-dependencies only; the `psd` library crate itself has no new
dependencies.

---

## Known limitations

These are things psd-to-json uses from psd-tools that this fork does not yet
cover:

1. **Vector mask clipping** -- `composite_rgba()` does not rasterize or clip by
   vector mask paths. Shape layers with vector-only masks will export their full
   bounding-box pixel content. The vector path data *is* exposed via
   `PsdLayer::vector_mask()`, so a caller can implement clipping downstream.

2. **Blend mode rendering** -- `Psd::flatten_layers_rgba()` uses
   one-minus-src-alpha blending regardless of layer blend mode. psd-to-json
   only serializes the blend mode name (it doesn't render composites with blend
   modes applied), so this does not block the port.

3. **16-bit / 32-bit depth and non-RGB color modes** -- the crate is tested
   against 8-bit RGB. Other depths and color modes (CMYK, Indexed, Grayscale)
   may produce incorrect pixels or errors.

4. **Layer visibility mutation** -- psd-to-json temporarily force-enables
   visibility during animation frame export. The Rust crate does not expose a
   setter for `visible`. A port can work around this by ignoring the visibility
   flag during animation rendering.
