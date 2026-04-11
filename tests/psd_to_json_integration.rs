//! End-to-end integration test that exercises every PSD feature the Python
//! project `psd-to-json` (https://github.com/laffan/psd-to-json) uses from
//! `psd-tools`, implemented against the Rust `psd` crate.
//!
//! The goal is to give a concrete, runnable smoke test that proves this fork
//! of `psd` can serve as the backbone for a Rust port of `psd-to-json`.
//!
//! For each fixture PSD this test:
//!
//!   1. Parses the PSD with `psd::Psd::from_bytes`
//!   2. Walks the layer tree (respecting groups/nesting) and builds a JSON
//!      document similar in shape to what `psd-to-json` emits
//!   3. Writes a per-PSD output directory containing:
//!        - `canvas.png`        full flattened document
//!        - `layers/<n>_<name>.png`   per-layer RGBA composites
//!        - `masks/<n>_<name>_mask.png`  per-layer raster masks (when present)
//!        - `data.json`         the JSON structure for that PSD
//!   4. Asserts a handful of known-good values so regressions are caught
//!
//! The outputs land in `$CARGO_TARGET_TMPDIR/<psd_name>/`. Cargo sets that
//! variable for integration tests; on a fresh checkout it resolves to
//! `target/tmp/psd_to_json_integration/<psd_name>/`.
//!
//! Run with:
//!
//!     cargo test --test psd_to_json_integration -- --nocapture
//!
//! The `--nocapture` flag lets the test print the absolute output path so you
//! can open the generated PNGs and JSON in a viewer.
//!
//! ## What psd-to-json uses from psd-tools (and the Rust equivalents)
//!
//! | psd-tools (Python)                    | Rust `psd` crate                    |
//! |---------------------------------------|-------------------------------------|
//! | `PSDImage.open(path)`                 | `Psd::from_bytes(&bytes)`           |
//! | `psd.width`, `psd.height`             | `Psd::width()`, `Psd::height()`     |
//! | iterate nested `.layers`              | `Psd::groups()` + layer `parent_id` |
//! | `layer.name`                          | `PsdLayer::name()`                  |
//! | `layer.left/top/right/bottom`         | `PsdLayer::layer_{left,top,...}()`  |
//! | `layer.width`, `layer.height`         | `PsdLayer::width()`, `height()`     |
//! | `layer.is_visible()`, `.visible`      | `PsdLayer::visible()`               |
//! | `layer.opacity` (0..255)              | `PsdLayer::opacity()`               |
//! | `layer.blend_mode`                    | `PsdLayer::blend_mode()`            |
//! | `layer.is_group()` / `isinstance`     | enumerate `Psd::groups()`           |
//! | `layer.composite()` -> PIL image      | `PsdLayer::rgba()`                  |
//! | `psd.composite()` / final render      | `Psd::flatten_layers_rgba()`        |
//! | `layer.mask.bbox`                     | `PsdLayer::mask()` -> `LayerMask`   |
//! | `layer.mask.topil()`                  | `PsdLayer::mask_pixels()`           |
//! | `layer.has_vector_mask()`             | `PsdLayer::has_vector_mask()`       |
//! | `layer.vector_mask.paths[...].knots`  | `PsdLayer::vector_mask()`           |
//!
//! psd-to-json never touches smart objects, text layers, layer effects or
//! styles, clip layers, or adjustment layers — so this test doesn't either.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use psd::{PsdGroup, PsdLayer};
use serde_json::{json, Value};

/// A single fixture to exercise, with a few quick assertions about the file.
struct Fixture {
    /// Short name used for the output directory.
    name: &'static str,
    /// PSD bytes (embedded via `include_bytes!`).
    bytes: &'static [u8],
    /// Optional expectations. `None` means "don't assert this value".
    expected_width: Option<u32>,
    expected_height: Option<u32>,
    expected_layer_count: Option<usize>,
    expected_group_count: Option<usize>,
    /// Layer-name -> expected parent group name (or `None` = top-level).
    expected_layer_parents: &'static [(&'static str, Option<&'static str>)],
}

/// The fixtures we exercise. These cover every PSD feature psd-to-json needs.
fn fixtures() -> Vec<Fixture> {
    vec![
        // Two layers, no groups — basic per-layer composite + bounds smoke test.
        Fixture {
            name: "two-layers-red-green-1x1",
            bytes: include_bytes!("fixtures/two-layers-red-green-1x1.psd"),
            expected_width: Some(1),
            expected_height: Some(1),
            expected_layer_count: Some(2),
            expected_group_count: Some(0),
            expected_layer_parents: &[],
        },
        // 3 layers at non-trivial size with RLE compression — checks that
        // decoded RGBA round-trips through PNG encoding.
        Fixture {
            name: "rle-3-layer-8x8",
            bytes: include_bytes!("fixtures/rle-3-layer-8x8.psd"),
            expected_width: Some(8),
            expected_height: Some(8),
            expected_layer_count: Some(3),
            expected_group_count: Some(0),
            expected_layer_parents: &[],
        },
        // Deeply nested groups — stress test of the group tree walker.
        Fixture {
            name: "one-group-with-two-subgroups",
            bytes: include_bytes!("fixtures/groups/green-1x1-one-group-with-two-subgroups.psd"),
            expected_width: Some(1),
            expected_height: Some(1),
            expected_layer_count: Some(6),
            expected_group_count: Some(6),
            expected_layer_parents: &[
                ("First Layer", Some("first group inside")),
                ("Second Layer", Some("sub sub group")),
                ("Third Layer", Some("second group inside")),
                ("Fourth Layer", Some("outside group")),
                ("Firth Layer", None),
                ("Sixth Layer", Some("outside group 2")),
            ],
        },
        // Visibility flags — psd-to-json reads `.visible` on every layer.
        Fixture {
            name: "visibility",
            bytes: include_bytes!("fixtures/visibility.psd"),
            expected_width: None,
            expected_height: None,
            expected_layer_count: None,
            expected_group_count: None,
            expected_layer_parents: &[],
        },
        // Partial transparency — opacity & alpha-aware per-layer rasterization.
        Fixture {
            name: "16x16-rle-partially-opaque",
            bytes: include_bytes!("fixtures/16x16-rle-partially-opaque.psd"),
            expected_width: Some(16),
            expected_height: Some(16),
            expected_layer_count: None,
            expected_group_count: None,
            expected_layer_parents: &[],
        },
        // Exercises both raster and vector mask parsing / export.
        Fixture {
            name: "vector-mask-and-layer-mask",
            bytes: include_bytes!("fixtures/vector-mask-and-layer-mask.psd"),
            expected_width: None,
            expected_height: None,
            expected_layer_count: None,
            expected_group_count: None,
            expected_layer_parents: &[],
        },
    ]
}

#[test]
fn psd_to_json_feature_smoke_test() -> Result<()> {
    let output_root = output_root()?;
    fs::create_dir_all(&output_root)?;
    eprintln!("\n[psd_to_json_integration] writing outputs to: {}\n", output_root.display());

    let mut summary: Vec<String> = Vec::new();

    for fixture in fixtures() {
        let fixture_dir = output_root.join(fixture.name);
        // Start from a clean slate each run so stale output can't mask a bug.
        if fixture_dir.exists() {
            fs::remove_dir_all(&fixture_dir)?;
        }
        fs::create_dir_all(fixture_dir.join("layers"))?;
        fs::create_dir_all(fixture_dir.join("masks"))?;

        let report = process_fixture(&fixture, &fixture_dir)
            .with_context(|| format!("processing fixture '{}'", fixture.name))?;
        summary.push(report);
    }

    eprintln!("\n[psd_to_json_integration] summary:");
    for line in summary {
        eprintln!("  {line}");
    }
    eprintln!();

    Ok(())
}

/// Parse a single fixture PSD, write outputs to `out_dir`, and run the
/// fixture's assertions. Returns a one-line summary.
fn process_fixture(fixture: &Fixture, out_dir: &Path) -> Result<String> {
    let psd = psd::Psd::from_bytes(fixture.bytes)
        .map_err(|e| anyhow::anyhow!("Psd::from_bytes failed: {e:?}"))?;

    let width = psd.width();
    let height = psd.height();

    if let Some(exp) = fixture.expected_width {
        assert_eq!(width, exp, "{}: width", fixture.name);
    }
    if let Some(exp) = fixture.expected_height {
        assert_eq!(height, exp, "{}: height", fixture.name);
    }
    if let Some(exp) = fixture.expected_layer_count {
        assert_eq!(psd.layers().len(), exp, "{}: layer count", fixture.name);
    }
    if let Some(exp) = fixture.expected_group_count {
        assert_eq!(psd.groups().len(), exp, "{}: group count", fixture.name);
    }
    for (layer_name, expected_parent) in fixture.expected_layer_parents {
        let layer = psd
            .layer_by_name(layer_name)
            .ok_or_else(|| anyhow::anyhow!("missing layer '{layer_name}'"))?;
        let actual_parent_name = layer
            .parent_id()
            .and_then(|id| psd.groups().get(&id).map(|g| g.name().to_string()));
        assert_eq!(
            actual_parent_name.as_deref(),
            *expected_parent,
            "{}: parent of layer '{}'",
            fixture.name,
            layer_name
        );
    }

    // --- 1. Full-canvas composite (equivalent to psd.composite() in psd-tools) ---
    let canvas_rgba = psd
        .flatten_layers_rgba(&|_| true)
        .map_err(|e| anyhow::anyhow!("flatten_layers_rgba failed: {e:?}"))?;
    write_rgba_png(&out_dir.join("canvas.png"), width, height, &canvas_rgba)
        .context("writing canvas.png")?;
    let canvas_bytes = fs::metadata(out_dir.join("canvas.png"))?.len();
    assert!(canvas_bytes > 0, "{}: canvas.png is empty", fixture.name);

    // --- 2. Per-layer composites (equivalent to layer.composite().save()) ---
    let mut layer_png_count = 0usize;
    let mut mask_png_count = 0usize;
    for (idx, layer) in psd.layers().iter().enumerate() {
        // Skip layers that have no pixel data (pure group markers can have
        // zero-sized bounds). The rgba() call on these would still yield an
        // all-transparent buffer, but there's no point writing a blank PNG.
        if layer.width() == 0 || layer.height() == 0 {
            continue;
        }
        let rgba = layer.rgba();
        // PsdLayer::rgba() produces a *full-canvas-sized* RGBA buffer with
        // the layer positioned at its offset. That matches what psd-to-json
        // gets back from `layer.composite()` (PIL with offset applied), so we
        // can write it directly.
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "{}: layer '{}' rgba unexpected size",
            fixture.name,
            layer.name()
        );
        let filename = format!("{:02}_{}.png", idx, sanitize(layer.name()));
        write_rgba_png(&out_dir.join("layers").join(&filename), width, height, &rgba)
            .with_context(|| format!("writing layer PNG {filename}"))?;
        layer_png_count += 1;

        // --- 3. Raster mask (equivalent to layer.mask.topil().save()) ---
        if let Some(mask_meta) = layer.mask() {
            if let Some(mask_pixels) = layer.mask_pixels() {
                let mw = mask_meta.width();
                let mh = mask_meta.height();
                if mw > 0 && mh > 0 && mask_pixels.len() == (mw * mh) as usize {
                    let mask_name = format!("{:02}_{}_mask.png", idx, sanitize(layer.name()));
                    write_gray_png(&out_dir.join("masks").join(&mask_name), mw, mh, &mask_pixels)
                        .with_context(|| format!("writing mask PNG {mask_name}"))?;
                    mask_png_count += 1;
                }
            }
        }
    }

    // --- 4. JSON document ---
    let data_json = build_json(&psd);
    let json_path = out_dir.join("data.json");
    fs::write(&json_path, serde_json::to_vec_pretty(&data_json)?)?;
    // Round-trip the JSON to make sure we produced something well-formed.
    let reparsed: Value = serde_json::from_slice(&fs::read(&json_path)?)?;
    assert_eq!(reparsed["width"], json!(width));
    assert_eq!(reparsed["height"], json!(height));
    assert!(reparsed["layers"].is_array(), "{}: layers is not array", fixture.name);

    // Fixture-specific JSON assertions.
    if fixture.name == "one-group-with-two-subgroups" {
        // Top-level children must appear in Photoshop order:
        //   outside group, Firth Layer, outside group 2
        let tops = reparsed["layers"].as_array().expect("layers array");
        let names: Vec<&str> = tops
            .iter()
            .map(|n| n["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec!["outside group", "Firth Layer", "outside group 2"],
            "top-level order"
        );
        // "outside group" must contain, in order: first group inside,
        // second group inside, Fourth Layer, third group inside.
        let outside = &tops[0];
        let kids = outside["children"].as_array().expect("children");
        let kid_names: Vec<&str> = kids
            .iter()
            .map(|n| n["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kid_names,
            vec![
                "first group inside",
                "second group inside",
                "Fourth Layer",
                "third group inside"
            ],
            "outside-group children order"
        );
    }
    if fixture.name == "vector-mask-and-layer-mask" {
        // At least one layer in this fixture has a vector_mask with >= 1 path.
        let tops = reparsed["layers"].as_array().unwrap();
        let has_vm = tops.iter().any(|n| {
            n.get("vector_mask")
                .and_then(|vm| vm["paths"].as_array())
                .map(|p| !p.is_empty())
                .unwrap_or(false)
        });
        assert!(has_vm, "expected at least one vector_mask path in JSON");
        // And at least one layer should carry a raster mask bbox.
        let has_mask = tops.iter().any(|n| n.get("mask").is_some());
        assert!(has_mask, "expected at least one raster mask in JSON");
    }

    Ok(format!(
        "{name}: {w}x{h}, {layers} layers ({layer_pngs} PNGs, {masks} masks), {groups} groups -> {dir}",
        name = fixture.name,
        w = width,
        h = height,
        layers = psd.layers().len(),
        layer_pngs = layer_png_count,
        masks = mask_png_count,
        groups = psd.groups().len(),
        dir = out_dir.display()
    ))
}

// ---------------------------------------------------------------------------
// JSON tree construction
// ---------------------------------------------------------------------------

/// Build a nested JSON representation of the PSD's layer tree, mirroring the
/// kind of document psd-to-json emits. The schema is intentionally close to
/// what psd-to-json writes so a downstream Rust port has a clear target.
fn build_json(psd: &psd::Psd) -> Value {
    let mut root = json!({
        "width": psd.width(),
        "height": psd.height(),
        "color_mode": format!("{:?}", psd.color_mode()),
        "depth": format!("{:?}", psd.depth()),
        "layers": Value::Array(vec![]),
    });

    // For each group, compute the minimum flat-layer index of any layer
    // whose ancestor chain contains that group. That index is then used as
    // the group's "position" among its siblings so groups and loose layers
    // sort into the same order Photoshop shows them.
    let mut group_min_idx: HashMap<u32, usize> = HashMap::new();
    for (idx, layer) in psd.layers().iter().enumerate() {
        let mut cur = layer.parent_id();
        while let Some(gid) = cur {
            group_min_idx
                .entry(gid)
                .and_modify(|v| *v = (*v).min(idx))
                .or_insert(idx);
            cur = psd.groups().get(&gid).and_then(|g| g.parent_id());
        }
    }
    // A group with no descendants at all still needs a key; fall back to a
    // very large value so it sorts to the end of its parent's children.
    let group_key = |gid: u32| -> usize {
        group_min_idx.get(&gid).copied().unwrap_or(usize::MAX)
    };

    // Index direct children for each group id (plus `None` = top-level).
    let mut children_by_group: HashMap<Option<u32>, Vec<Child>> = HashMap::new();
    for (idx, layer) in psd.layers().iter().enumerate() {
        children_by_group
            .entry(layer.parent_id())
            .or_default()
            .push(Child::Layer(idx));
    }
    for (&group_id, group) in psd.groups() {
        children_by_group
            .entry(group.parent_id())
            .or_default()
            .push(Child::Group(group_id));
    }

    // Sort each bucket so loose layers and groups interleave in document order.
    for children in children_by_group.values_mut() {
        children.sort_by_key(|c| match c {
            Child::Layer(idx) => *idx,
            Child::Group(gid) => group_key(*gid),
        });
    }

    let tops = children_by_group
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .map(|c| child_to_json(psd, c, &children_by_group, 0))
        .collect();
    root["layers"] = Value::Array(tops);
    root
}

/// A direct child of either the PSD root or a group.
#[derive(Clone, Copy)]
enum Child {
    Layer(usize),
    Group(u32),
}

fn child_to_json(
    psd: &psd::Psd,
    child: Child,
    children_by_group: &HashMap<Option<u32>, Vec<Child>>,
    depth: usize,
) -> Value {
    match child {
        Child::Layer(idx) => layer_to_json(psd.layer_by_idx(idx), idx, depth),
        Child::Group(gid) => {
            let group = &psd.groups()[&gid];
            let kids: Vec<Value> = children_by_group
                .get(&Some(gid))
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|c| child_to_json(psd, c, children_by_group, depth + 1))
                .collect();
            group_to_json(group, gid, depth, kids)
        }
    }
}

fn layer_to_json(layer: &PsdLayer, idx: usize, depth: usize) -> Value {
    let mut attrs: BTreeMap<&str, Value> = BTreeMap::new();
    attrs.insert("kind", json!("layer"));
    attrs.insert("index", json!(idx));
    attrs.insert("depth", json!(depth));
    attrs.insert("name", json!(layer.name()));
    attrs.insert("visible", json!(layer.visible()));
    attrs.insert("opacity", json!(layer.opacity()));
    attrs.insert("alpha", json!(layer.opacity() as f64 / 255.0));
    attrs.insert("blend_mode", json!(format!("{:?}", layer.blend_mode())));
    attrs.insert("clipping_mask", json!(layer.is_clipping_mask()));
    attrs.insert("x", json!(layer.layer_left()));
    attrs.insert("y", json!(layer.layer_top()));
    attrs.insert("width", json!(layer.width()));
    attrs.insert("height", json!(layer.height()));
    attrs.insert(
        "bbox",
        json!({
            "left":   layer.layer_left(),
            "top":    layer.layer_top(),
            "right":  layer.layer_right(),
            "bottom": layer.layer_bottom(),
        }),
    );

    if let Some(mask) = layer.mask() {
        attrs.insert(
            "mask",
            json!({
                "left":   mask.left,
                "top":    mask.top,
                "right":  mask.right,
                "bottom": mask.bottom,
                "width":  mask.width(),
                "height": mask.height(),
                "default_color":     mask.default_color,
                "disabled":          mask.disabled,
                "invert":            mask.invert,
                "relative_to_layer": mask.relative_to_layer,
            }),
        );
    }

    if layer.has_vector_mask() {
        if let Some(vm) = layer.vector_mask() {
            let paths: Vec<Value> = vm
                .paths
                .iter()
                .map(|sp| {
                    let knots: Vec<Value> = sp
                        .knots
                        .iter()
                        .map(|k| {
                            json!({
                                "anchor":    [k.anchor.x, k.anchor.y],
                                "preceding": [k.preceding.x, k.preceding.y],
                                "following": [k.following.x, k.following.y],
                                "linked":    k.linked,
                            })
                        })
                        .collect();
                    json!({ "closed": sp.closed, "knots": knots })
                })
                .collect();
            attrs.insert(
                "vector_mask",
                json!({
                    "disabled":   vm.disabled,
                    "invert":     vm.invert,
                    "not_linked": vm.not_linked,
                    "paths":      paths,
                }),
            );
        }
    }

    Value::Object(attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn group_to_json(group: &PsdGroup, gid: u32, depth: usize, children: Vec<Value>) -> Value {
    let mut attrs: BTreeMap<&str, Value> = BTreeMap::new();
    attrs.insert("kind", json!("group"));
    attrs.insert("id", json!(gid));
    attrs.insert("depth", json!(depth));
    attrs.insert("name", json!(group.name()));
    attrs.insert("visible", json!(group.visible()));
    attrs.insert("opacity", json!(group.opacity()));
    attrs.insert("alpha", json!(group.opacity() as f64 / 255.0));
    attrs.insert("blend_mode", json!(format!("{:?}", group.blend_mode())));
    attrs.insert("x", json!(group.layer_left()));
    attrs.insert("y", json!(group.layer_top()));
    attrs.insert("width", json!(group.width()));
    attrs.insert("height", json!(group.height()));
    attrs.insert("children", Value::Array(children));
    Value::Object(attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

// ---------------------------------------------------------------------------
// PNG writers
// ---------------------------------------------------------------------------

fn write_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    assert_eq!(
        rgba.len(),
        (width * height * 4) as usize,
        "RGBA buffer size mismatch for {}",
        path.display()
    );
    let file = fs::File::create(path)?;
    let w = &mut std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

fn write_gray_png(path: &Path, width: u32, height: u32, gray: &[u8]) -> Result<()> {
    assert_eq!(
        gray.len(),
        (width * height) as usize,
        "grayscale buffer size mismatch for {}",
        path.display()
    );
    let file = fs::File::create(path)?;
    let w = &mut std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(gray)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the per-test output directory. Prefer the Cargo-provided tmp dir
/// for integration tests; fall back to `<manifest_dir>/target/psd-to-json-out`
/// if that variable is missing (e.g. running under a non-standard harness).
fn output_root() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("CARGO_TARGET_TMPDIR") {
        return Ok(PathBuf::from(dir));
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set")?;
    Ok(PathBuf::from(manifest).join("target").join("psd-to-json-out"))
}

/// Make a layer name safe to use as a file name on disk.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}
