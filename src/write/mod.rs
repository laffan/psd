//! Create new PSD files.
//!
//! The entry point is [`PsdBuilder`]. You give it a canvas size and a stack of
//! [`LayerBuilder`]s and [`GroupBuilder`]s and it hands you back the bytes of a
//! PSD file that Photoshop - and [`Psd::from_bytes`](crate::Psd::from_bytes) -
//! can open.
//!
//! ```
//! use psd::{GroupBuilder, LayerBuilder, Psd, PsdBuilder};
//!
//! // A 2x2 canvas with an opaque red background and a half transparent blue
//! // square nested two groups deep.
//! let mut psd = PsdBuilder::new(2, 2);
//!
//! psd.add_layer(LayerBuilder::new("Background").rgba(2, 2, vec![255, 0, 0, 255].repeat(4)));
//!
//! let inner = GroupBuilder::new("Inner")
//!     .add_layer(LayerBuilder::new("Square").rgba(1, 1, vec![0, 0, 255, 128]).at(1, 1));
//! psd.add_group(GroupBuilder::new("Outer").add_group(inner));
//!
//! let bytes = psd.to_bytes().unwrap();
//!
//! let psd = Psd::from_bytes(&bytes).unwrap();
//! assert_eq!(psd.layers().len(), 2);
//! assert_eq!(psd.groups().len(), 2);
//! ```
//!
//! # What gets written
//!
//! We write 8 bit RGB PSDs. Every layer is stored with an alpha channel plus
//! red, green and blue channels, which is what Photoshop itself writes for a
//! non-background layer.
//!
//! The colour mode data and image resources sections are written empty, and
//! layers are written without masks. If you need something that isn't written
//! here please open an issue.

use thiserror::Error;

use crate::psd_channel::{PsdChannelCompression, PsdChannelKind};
use crate::sections::file_header_section::{ColorMode, PsdDepth, EXPECTED_PSD_SIGNATURE};
use crate::sections::layer_and_mask_information_section::layer::BlendMode;
use crate::write::bytes::ByteWriter;
use crate::write::rle::pack_bits;

mod bytes;
mod composite;
mod rle;

/// The signature that prefixes a blend mode key and every additional layer
/// information block.
const EIGHT_BIM: &[u8; 4] = b"8BIM";

/// Photoshop names every bounding section divider record this.
const GROUP_DIVIDER_NAME: &str = "</Layer group>";

/// The largest width or height that the file header can describe.
const MAX_CANVAS_DIMENSION: u32 = 30_000;

/// A PSD stores its layer count in an i16.
const MAX_LAYER_RECORDS: usize = i16::MAX as usize;

/// The order that Photoshop writes a layer's channels in.
const LAYER_CHANNEL_ORDER: [PsdChannelKind; 4] = [
    PsdChannelKind::TransparencyMask,
    PsdChannelKind::Red,
    PsdChannelKind::Green,
    PsdChannelKind::Blue,
];

/// The reasons that we might not be able to turn a [`PsdBuilder`] into bytes.
///
/// This list is intended to grow over time and it is not recommended to
/// exhaustively match against it.
#[derive(Debug, PartialEq, Error)]
#[non_exhaustive]
pub enum PsdWriteError {
    /// The canvas is outside of the range that a PSD file header can describe.
    #[error("A PSD must be between 1x1 and 30,000x30,000. You provided {width}x{height}.")]
    InvalidCanvasSize {
        /// The canvas width that was provided
        width: u32,
        /// The canvas height that was provided
        height: u32,
    },
    /// A layer is bigger than the largest canvas that a PSD can describe.
    #[error("Layer '{layer_name}' is {width}x{height}. Layers can be at most 30,000x30,000.")]
    LayerTooLarge {
        /// The name of the offending layer
        layer_name: String,
        /// The layer width that was provided
        width: u32,
        /// The layer height that was provided
        height: u32,
    },
    /// A layer's pixels did not match the layer's dimensions.
    #[error(
        r#"Layer '{layer_name}' is {width}x{height}, so it needs {expected} RGBA bytes.
        You provided {actual}."#
    )]
    LayerPixelCountMismatch {
        /// The name of the offending layer
        layer_name: String,
        /// The layer width that was provided
        width: u32,
        /// The layer height that was provided
        height: u32,
        /// The number of bytes that the layer's dimensions call for
        expected: usize,
        /// The number of bytes that were provided
        actual: usize,
    },
    /// A hand written flattened image did not match the canvas dimensions.
    #[error("The flattened image needs {expected} RGBA bytes. You provided {actual}.")]
    FlattenedImagePixelCountMismatch {
        /// The number of bytes that the canvas dimensions call for
        expected: usize,
        /// The number of bytes that were provided
        actual: usize,
    },
    /// More layer and group records than a PSD's i16 layer count can address.
    #[error(
        r#"A PSD can contain at most {max} layer records. Note that every group
        costs two records - one to open it and one to close it. You provided {count}."#
    )]
    TooManyLayerRecords {
        /// The number of records that the layer stack works out to
        count: usize,
        /// The largest number of records that a PSD can hold
        max: usize,
    },
    /// We can only write raw and RLE compressed channels.
    #[error("{compression:#?} channels cannot be written. Use RawData or RleCompressed.")]
    UnsupportedCompression {
        /// The compression that was asked for
        compression: PsdChannelCompression,
    },
}

/// A pixel layer to write into a new PSD.
///
/// ```
/// use psd::{BlendMode, LayerBuilder};
///
/// let layer = LayerBuilder::new("Hero")
///     .rgba(1, 1, vec![0, 255, 0, 255])
///     .at(10, 20)
///     .opacity(128)
///     .blend_mode(BlendMode::Multiply);
/// ```
#[derive(Debug, Clone)]
pub struct LayerBuilder {
    name: String,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    opacity: u8,
    visible: bool,
    blend_mode: BlendMode,
    clipped: bool,
}

impl LayerBuilder {
    /// Create a new, empty layer.
    ///
    /// Give it pixels with [`LayerBuilder::rgba`]. A layer with no pixels is
    /// valid - Photoshop shows it as an empty layer.
    pub fn new(name: impl Into<String>) -> LayerBuilder {
        LayerBuilder {
            name: name.into(),
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            rgba: vec![],
            opacity: 255,
            visible: true,
            blend_mode: BlendMode::Normal,
            clipped: false,
        }
    }

    /// Set the layer's pixels.
    ///
    /// `rgba` is `[R, G, B, A, R, G, B, A, ...]` in the same layout that
    /// [`PsdLayer::rgba`](crate::PsdLayer::rgba) returns, and must be exactly
    /// `width * height * 4` bytes long.
    pub fn rgba(mut self, width: u32, height: u32, rgba: Vec<u8>) -> LayerBuilder {
        self.width = width;
        self.height = height;
        self.rgba = rgba;

        self
    }

    /// Position the layer's top left corner within the canvas.
    ///
    /// Defaults to `(0, 0)`. Negative coordinates and coordinates that push the
    /// layer past the edge of the canvas are allowed - Photoshop keeps the
    /// pixels that hang off of the canvas.
    pub fn at(mut self, left: i32, top: i32) -> LayerBuilder {
        self.left = left;
        self.top = top;

        self
    }

    /// Set the layer's opacity. 0 is fully transparent, 255 is fully opaque.
    ///
    /// Defaults to 255.
    pub fn opacity(mut self, opacity: u8) -> LayerBuilder {
        self.opacity = opacity;

        self
    }

    /// Set whether the layer's eyeball is toggled on in Photoshop.
    ///
    /// Defaults to `true`.
    pub fn visible(mut self, visible: bool) -> LayerBuilder {
        self.visible = visible;

        self
    }

    /// Set how this layer blends with the layers below it.
    ///
    /// Defaults to [`BlendMode::Normal`].
    pub fn blend_mode(mut self, blend_mode: BlendMode) -> LayerBuilder {
        self.blend_mode = blend_mode;

        self
    }

    /// Clip this layer to the layer below it.
    ///
    /// Defaults to `false`.
    ///
    /// # Note
    ///
    /// [`PsdLayer::is_clipping_mask`](crate::PsdLayer::is_clipping_mask)
    /// reports the opposite of this flag - it is `true` for layers that other
    /// layers can be clipped to. Parsing a PSD that we wrote will therefore
    /// give `is_clipping_mask() == !clipped_to_layer_below`.
    pub fn clipped_to_layer_below(mut self, clipped: bool) -> LayerBuilder {
        self.clipped = clipped;

        self
    }

    /// The number of RGBA bytes that this layer's dimensions call for.
    fn expected_pixel_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// A layer with no pixels is written with the same zeroed rectangle that
    /// Photoshop uses for its own empty records.
    fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Pull one of the four interleaved RGBA channels out into its own plane.
    fn channel_plane(&self, channel: PsdChannelKind) -> Vec<u8> {
        if self.is_empty() {
            return vec![];
        }

        let offset = match channel {
            PsdChannelKind::Red => 0,
            PsdChannelKind::Green => 1,
            PsdChannelKind::Blue => 2,
            _ => 3,
        };

        self.rgba.iter().skip(offset).step_by(4).copied().collect()
    }
}

/// A group of layers to write into a new PSD.
///
/// Groups can contain layers and other groups, so a group is how you nest.
///
/// ```
/// use psd::{GroupBuilder, LayerBuilder};
///
/// let group = GroupBuilder::new("Characters")
///     .add_layer(LayerBuilder::new("Hero").rgba(1, 1, vec![0, 255, 0, 255]))
///     .add_group(GroupBuilder::new("Props"));
/// ```
#[derive(Debug, Clone)]
pub struct GroupBuilder {
    name: String,
    children: Vec<Child>,
    opacity: u8,
    visible: bool,
    blend_mode: BlendMode,
    collapsed: bool,
}

impl GroupBuilder {
    /// Create a new, empty group.
    pub fn new(name: impl Into<String>) -> GroupBuilder {
        GroupBuilder {
            name: name.into(),
            children: vec![],
            opacity: 255,
            visible: true,
            blend_mode: BlendMode::PassThrough,
            collapsed: false,
        }
    }

    /// Put a layer on top of this group's current contents.
    pub fn add_layer(mut self, layer: LayerBuilder) -> GroupBuilder {
        self.push_layer(layer);

        self
    }

    /// Put a group on top of this group's current contents.
    pub fn add_group(mut self, group: GroupBuilder) -> GroupBuilder {
        self.push_group(group);

        self
    }

    /// Put a layer on top of this group's current contents.
    ///
    /// The same as [`GroupBuilder::add_layer`], for when you're adding layers
    /// in a loop and a chainable method would get in the way.
    pub fn push_layer(&mut self, layer: LayerBuilder) {
        self.children.push(Child::Layer(layer));
    }

    /// Put a group on top of this group's current contents.
    ///
    /// The same as [`GroupBuilder::add_group`], for when you're adding groups
    /// in a loop and a chainable method would get in the way.
    pub fn push_group(&mut self, group: GroupBuilder) {
        self.children.push(Child::Group(group));
    }

    /// Set the group's opacity. 0 is fully transparent, 255 is fully opaque.
    ///
    /// Defaults to 255.
    pub fn opacity(mut self, opacity: u8) -> GroupBuilder {
        self.opacity = opacity;

        self
    }

    /// Set whether the group's eyeball is toggled on in Photoshop.
    ///
    /// Defaults to `true`.
    pub fn visible(mut self, visible: bool) -> GroupBuilder {
        self.visible = visible;

        self
    }

    /// Set how this group blends with the layers below it.
    ///
    /// Defaults to [`BlendMode::PassThrough`], which is Photoshop's default for
    /// a new group.
    pub fn blend_mode(mut self, blend_mode: BlendMode) -> GroupBuilder {
        self.blend_mode = blend_mode;

        self
    }

    /// Set whether the group's folder starts out folded shut in Photoshop's
    /// layers panel.
    ///
    /// Defaults to `false`.
    pub fn collapsed(mut self, collapsed: bool) -> GroupBuilder {
        self.collapsed = collapsed;

        self
    }
}

/// One entry in a layer stack.
#[derive(Debug, Clone)]
enum Child {
    Layer(LayerBuilder),
    Group(GroupBuilder),
}

/// Builds the bytes of a new PSD file.
///
/// Layers and groups are added from the bottom of the layer stack upwards, so
/// the first thing you add is the bottom-most thing in the document.
///
/// ```
/// use psd::{LayerBuilder, PsdBuilder};
///
/// let mut psd = PsdBuilder::new(1, 1);
/// psd.add_layer(LayerBuilder::new("Bottom").rgba(1, 1, vec![255, 0, 0, 255]));
/// psd.add_layer(LayerBuilder::new("Top").rgba(1, 1, vec![0, 0, 255, 255]));
///
/// let bytes: Vec<u8> = psd.to_bytes().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct PsdBuilder {
    width: u32,
    height: u32,
    children: Vec<Child>,
    compression: PsdChannelCompression,
    flattened_image: Option<Vec<u8>>,
}

impl PsdBuilder {
    /// Create a builder for a `width` by `height` PSD.
    pub fn new(width: u32, height: u32) -> PsdBuilder {
        PsdBuilder {
            width,
            height,
            children: vec![],
            compression: PsdChannelCompression::RleCompressed,
            flattened_image: None,
        }
    }

    /// Put a layer on top of the PSD's current contents.
    pub fn add_layer(&mut self, layer: LayerBuilder) -> &mut PsdBuilder {
        self.children.push(Child::Layer(layer));

        self
    }

    /// Put a group on top of the PSD's current contents.
    pub fn add_group(&mut self, group: GroupBuilder) -> &mut PsdBuilder {
        self.children.push(Child::Group(group));

        self
    }

    /// Choose how channel data is compressed.
    ///
    /// Defaults to [`PsdChannelCompression::RleCompressed`], which is what
    /// Photoshop writes. [`PsdChannelCompression::RawData`] writes faster and
    /// produces bigger files. The two ZIP compressions are not supported.
    pub fn compression(&mut self, compression: PsdChannelCompression) -> &mut PsdBuilder {
        self.compression = compression;

        self
    }

    /// Provide the flattened image that goes in the PSD's image data section.
    ///
    /// `rgba` must be `width * height * 4` bytes long, with straight (not
    /// premultiplied) alpha. We composite it over white on the way out, the
    /// same as Photoshop does.
    ///
    /// If you don't call this we composite the layers ourselves. See
    /// [`PsdBuilder::to_bytes`] for the caveats that come with that.
    pub fn flattened_image(&mut self, rgba: Vec<u8>) -> &mut PsdBuilder {
        self.flattened_image = Some(rgba);

        self
    }

    /// Serialize into the bytes of a PSD file.
    ///
    /// # The flattened image
    ///
    /// A PSD's last section holds a flattened copy of the document, which is
    /// what image viewers and [`Psd::rgba`](crate::Psd::rgba) read. Unless you
    /// supplied one with [`PsdBuilder::flattened_image`] we composite the
    /// layers ourselves using ordinary source-over alpha compositing, honouring
    /// each layer's and group's opacity and visibility but *not* its blend
    /// mode. Blend modes are still written into the layer records, so Photoshop
    /// re-composites the document correctly as soon as it opens the file.
    ///
    /// When the document has any transparency in it we write the flattened
    /// image the way that Photoshop does - an alpha channel, plus colour
    /// channels that have already been composited over white. A fully
    /// transparent pixel is therefore stored as white, not as black.
    pub fn to_bytes(&self) -> Result<Vec<u8>, PsdWriteError> {
        self.validate()?;

        let flattened_image = match &self.flattened_image {
            Some(flattened_image) => flattened_image.clone(),
            None => composite::flatten(self.width, self.height, &self.children),
        };

        // Photoshop only writes an alpha channel for the flattened image when
        // the document has some transparency in it.
        let has_transparency = flattened_image.iter().skip(3).step_by(4).any(|a| *a != 255);
        let channel_count = if has_transparency { 4 } else { 3 };

        let flattened_image = if has_transparency {
            composite::matte_over_white(&flattened_image)
        } else {
            flattened_image
        };

        let mut psd = ByteWriter::with_capacity(flattened_image.len());

        self.write_file_header_section(&mut psd, channel_count);

        // Colour mode data section. Only indexed and duotone PSDs put anything
        // in here.
        psd.u32(0);

        // Image resources section.
        psd.u32(0);

        self.write_layer_and_mask_information_section(&mut psd, has_transparency);
        self.write_image_data_section(&mut psd, &flattened_image, channel_count);

        Ok(psd.into_bytes())
    }

    /// Check everything that would otherwise produce a PSD that can't be read
    /// back.
    fn validate(&self) -> Result<(), PsdWriteError> {
        if self.width < 1
            || self.height < 1
            || self.width > MAX_CANVAS_DIMENSION
            || self.height > MAX_CANVAS_DIMENSION
        {
            return Err(PsdWriteError::InvalidCanvasSize {
                width: self.width,
                height: self.height,
            });
        }

        match self.compression {
            PsdChannelCompression::RawData | PsdChannelCompression::RleCompressed => {}
            compression => return Err(PsdWriteError::UnsupportedCompression { compression }),
        }

        if let Some(flattened_image) = &self.flattened_image {
            let expected = self.width as usize * self.height as usize * 4;
            if flattened_image.len() != expected {
                return Err(PsdWriteError::FlattenedImagePixelCountMismatch {
                    expected,
                    actual: flattened_image.len(),
                });
            }
        }

        let record_count = validate_children(&self.children)?;
        if record_count > MAX_LAYER_RECORDS {
            return Err(PsdWriteError::TooManyLayerRecords {
                count: record_count,
                max: MAX_LAYER_RECORDS,
            });
        }

        Ok(())
    }

    /// The first 26 bytes of the file.
    fn write_file_header_section(&self, psd: &mut ByteWriter, channel_count: u16) {
        psd.bytes(&EXPECTED_PSD_SIGNATURE);
        // Version. PSB would be 2.
        psd.u16(1);
        psd.zeros(6);
        psd.u16(channel_count);
        psd.u32(self.height);
        psd.u32(self.width);
        psd.u16(PsdDepth::Eight as u16);
        psd.u16(ColorMode::Rgb as u16);
    }

    /// The layer records, the pixels that they point at, and the length markers
    /// that wrap them.
    fn write_layer_and_mask_information_section(
        &self,
        psd: &mut ByteWriter,
        has_transparency: bool,
    ) {
        if self.children.is_empty() {
            // A PSD with no layers is just a length of zero.
            psd.u32(0);

            return;
        }

        // Layer records and the channel data that they describe are written as
        // two runs of bytes, so we build them side by side.
        let mut records = ByteWriter::new();
        let mut channel_data = ByteWriter::new();
        let mut record_count = 0;

        self.write_children(
            &self.children,
            &mut records,
            &mut channel_data,
            &mut record_count,
        );

        let mut layer_info = ByteWriter::with_capacity(records.len() + channel_data.len() + 2);

        // A negative layer count tells a reader that the flattened image's
        // first alpha channel is the document's transparency.
        let layer_count = record_count as i16;
        layer_info.i16(if has_transparency {
            -layer_count
        } else {
            layer_count
        });

        layer_info.bytes(records.as_slice());
        layer_info.bytes(channel_data.as_slice());
        layer_info.pad_to_multiple_of(2);

        let mut layer_and_mask = ByteWriter::with_capacity(layer_info.len() + 8);
        layer_and_mask.length_prefixed(layer_info.as_slice());
        // Global layer mask info. We don't write one.
        layer_and_mask.u32(0);

        psd.length_prefixed(layer_and_mask.as_slice());
    }

    /// Write a stack of layers and groups, bottom-most first, which is the
    /// order that a PSD stores its layer records in.
    fn write_children(
        &self,
        children: &[Child],
        records: &mut ByteWriter,
        channel_data: &mut ByteWriter,
        record_count: &mut usize,
    ) {
        for child in children {
            match child {
                Child::Layer(layer) => {
                    self.write_layer_record(layer, records, channel_data);
                    *record_count += 1;
                }
                Child::Group(group) => {
                    // A group is three parts, bottom to top: a hidden record
                    // that closes the folder, the group's contents, and a
                    // record that opens the folder.
                    self.write_divider_record(records, channel_data);
                    *record_count += 1;

                    self.write_children(&group.children, records, channel_data, record_count);

                    self.write_folder_record(group, records, channel_data);
                    *record_count += 1;
                }
            }
        }
    }

    /// A record for a layer that has pixels.
    fn write_layer_record(
        &self,
        layer: &LayerBuilder,
        records: &mut ByteWriter,
        channel_data: &mut ByteWriter,
    ) {
        let rectangle = if layer.is_empty() {
            // Photoshop writes a zeroed rectangle for records with no pixels.
            (0, 0, 0, 0)
        } else {
            (
                layer.top,
                layer.left,
                layer.top + layer.height as i32,
                layer.left + layer.width as i32,
            )
        };

        let channels: Vec<(PsdChannelKind, PsdChannelCompression, Vec<u8>)> = LAYER_CHANNEL_ORDER
            .iter()
            .map(|channel| {
                let (compression, data) = encode_channel(
                    &layer.channel_plane(*channel),
                    layer.width as usize,
                    layer.height as usize,
                    self.compression,
                );

                (*channel, compression, data)
            })
            .collect();

        write_record(
            records,
            channel_data,
            Record {
                rectangle,
                name: &layer.name,
                opacity: layer.opacity,
                visible: layer.visible,
                blend_mode: layer.blend_mode,
                clipped: layer.clipped,
                divider: None,
                channels: &channels,
            },
        );
    }

    /// The hidden record that marks the bottom of a group.
    fn write_divider_record(&self, records: &mut ByteWriter, channel_data: &mut ByteWriter) {
        write_record(
            records,
            channel_data,
            Record {
                rectangle: (0, 0, 0, 0),
                name: GROUP_DIVIDER_NAME,
                // Photoshop always writes a fully opaque, visible, normally
                // blended divider. The group's real settings live on the record
                // that opens the folder.
                opacity: 255,
                visible: true,
                blend_mode: BlendMode::Normal,
                clipped: false,
                divider: Some(Divider::BoundingSection),
                channels: &empty_channels(),
            },
        );
    }

    /// The record that names a group and holds its settings.
    fn write_folder_record(
        &self,
        group: &GroupBuilder,
        records: &mut ByteWriter,
        channel_data: &mut ByteWriter,
    ) {
        let divider = if group.collapsed {
            Divider::ClosedFolder
        } else {
            Divider::OpenFolder
        };

        write_record(
            records,
            channel_data,
            Record {
                rectangle: (0, 0, 0, 0),
                name: &group.name,
                opacity: group.opacity,
                visible: group.visible,
                blend_mode: group.blend_mode,
                clipped: false,
                divider: Some(divider),
                channels: &empty_channels(),
            },
        );
    }

    /// The flattened image that sits at the end of the file.
    fn write_image_data_section(&self, psd: &mut ByteWriter, rgba: &[u8], channel_count: u16) {
        let width = self.width as usize;
        let height = self.height as usize;

        let planes: Vec<Vec<u8>> = (0..channel_count as usize)
            .map(|offset| rgba.iter().skip(offset).step_by(4).copied().collect())
            .collect();

        match self.compression {
            PsdChannelCompression::RleCompressed => {
                psd.u16(PsdChannelCompression::RleCompressed as u16);

                // Every scanline's compressed length comes first, for all of
                // the channels, and then all of the compressed scanlines.
                let packed: Vec<Vec<Vec<u8>>> = planes
                    .iter()
                    .map(|plane| {
                        (0..height)
                            .map(|row| pack_bits(&plane[row * width..(row + 1) * width]))
                            .collect()
                    })
                    .collect();

                for plane in &packed {
                    for scanline in plane {
                        psd.u16(scanline.len() as u16);
                    }
                }

                for plane in &packed {
                    for scanline in plane {
                        psd.bytes(scanline);
                    }
                }
            }
            _ => {
                psd.u16(PsdChannelCompression::RawData as u16);

                for plane in &planes {
                    psd.bytes(plane);
                }
            }
        }
    }
}

/// Walk a layer stack, checking every layer and counting the records that it
/// will take to write it.
fn validate_children(children: &[Child]) -> Result<usize, PsdWriteError> {
    let mut record_count = 0;

    for child in children {
        match child {
            Child::Layer(layer) => {
                if layer.width > MAX_CANVAS_DIMENSION || layer.height > MAX_CANVAS_DIMENSION {
                    return Err(PsdWriteError::LayerTooLarge {
                        layer_name: layer.name.clone(),
                        width: layer.width,
                        height: layer.height,
                    });
                }

                let expected = layer.expected_pixel_bytes();
                if layer.rgba.len() != expected {
                    return Err(PsdWriteError::LayerPixelCountMismatch {
                        layer_name: layer.name.clone(),
                        width: layer.width,
                        height: layer.height,
                        expected,
                        actual: layer.rgba.len(),
                    });
                }

                record_count += 1;
            }
            Child::Group(group) => {
                // One record to open the folder and one to close it.
                record_count += 2 + validate_children(&group.children)?;
            }
        }
    }

    Ok(record_count)
}

/// The `lsct` section divider setting that a record carries.
#[derive(Debug, Clone, Copy)]
enum Divider {
    OpenFolder = 1,
    ClosedFolder = 2,
    BoundingSection = 3,
}

/// Everything that goes into one layer record.
struct Record<'a> {
    /// (top, left, bottom, right). Bottom and right are exclusive, the way that
    /// a PSD stores them.
    rectangle: (i32, i32, i32, i32),
    name: &'a str,
    opacity: u8,
    visible: bool,
    blend_mode: BlendMode,
    clipped: bool,
    divider: Option<Divider>,
    channels: &'a [(PsdChannelKind, PsdChannelCompression, Vec<u8>)],
}

/// Write one layer record, along with the channel data that it points at.
fn write_record(records: &mut ByteWriter, channel_data: &mut ByteWriter, record: Record) {
    let (top, left, bottom, right) = record.rectangle;

    records.i32(top);
    records.i32(left);
    records.i32(bottom);
    records.i32(right);

    records.u16(record.channels.len() as u16);
    for (channel, compression, data) in record.channels {
        records.i16(*channel as i16);
        // The channel's length includes the two bytes that describe how it is
        // compressed.
        records.u32(2 + data.len() as u32);

        channel_data.u16(*compression as u16);
        channel_data.bytes(data);
    }

    records.bytes(EIGHT_BIM);
    records.bytes(&record.blend_mode.key());

    records.u8(record.opacity);
    // Clipping. 0 = base, 1 = non-base.
    records.u8(record.clipped as u8);
    records.u8(record_flags(record.visible, record.divider.is_some()));
    // Filler.
    records.u8(0);

    let mut extra = ByteWriter::new();

    // Layer mask data. We don't write masks.
    extra.u32(0);
    // Layer blending ranges.
    extra.u32(0);

    extra.pascal_string(record.name, 4);

    write_additional_layer_information(&mut extra, b"luni", &{
        let mut luni = ByteWriter::new();
        luni.unicode_string(record.name);
        luni.pad_to_multiple_of(4);
        luni.into_bytes()
    });

    if let Some(divider) = record.divider {
        let mut lsct = ByteWriter::new();
        lsct.u32(divider as u32);

        // The record that opens a folder also carries the group's blend mode.
        // The hidden record that closes one is only ever four bytes long.
        if let Divider::OpenFolder | Divider::ClosedFolder = divider {
            lsct.bytes(EIGHT_BIM);
            lsct.bytes(&record.blend_mode.key());
            // Sub type. 0 = normal, 1 = scene group.
            lsct.u32(0);
        }

        write_additional_layer_information(&mut extra, b"lsct", lsct.as_slice());
    }

    records.length_prefixed(extra.as_slice());
}

/// Write one `8BIM` prefixed additional layer information block.
fn write_additional_layer_information(extra: &mut ByteWriter, key: &[u8; 4], data: &[u8]) {
    extra.bytes(EIGHT_BIM);
    extra.bytes(key);
    extra.length_prefixed(data);
}

/// Build a layer record's flags byte.
///
/// - bit 0 = transparency protected
/// - bit 1 = the layer is *hidden*
/// - bit 2 = obsolete
/// - bit 3 = 1 for Photoshop 5.0 and later, tells if bit 4 has useful information
/// - bit 4 = pixel data irrelevant to appearance of document
fn record_flags(visible: bool, pixel_data_irrelevant: bool) -> u8 {
    let mut flags = 1 << 3;

    if !visible {
        flags |= 1 << 1;
    }

    if pixel_data_irrelevant {
        flags |= 1 << 4;
    }

    flags
}

/// The four empty channels that a group's records are written with.
fn empty_channels() -> Vec<(PsdChannelKind, PsdChannelCompression, Vec<u8>)> {
    LAYER_CHANNEL_ORDER
        .iter()
        .map(|channel| (*channel, PsdChannelCompression::RawData, vec![]))
        .collect()
}

/// Compress one channel of a layer.
///
/// Returns the compression that was actually used along with the bytes to write
/// after the channel's two compression bytes. RLE compressed channels are
/// prefixed with the compressed length of each of their scanlines.
fn encode_channel(
    plane: &[u8],
    width: usize,
    height: usize,
    compression: PsdChannelCompression,
) -> (PsdChannelCompression, Vec<u8>) {
    if plane.is_empty() {
        return (PsdChannelCompression::RawData, vec![]);
    }

    match compression {
        PsdChannelCompression::RleCompressed => {
            let scanlines: Vec<Vec<u8>> = (0..height)
                .map(|row| pack_bits(&plane[row * width..(row + 1) * width]))
                .collect();

            let packed_len: usize = scanlines.iter().map(|scanline| scanline.len()).sum();

            let mut data = ByteWriter::with_capacity(2 * height + packed_len);
            for scanline in &scanlines {
                data.u16(scanline.len() as u16);
            }
            for scanline in &scanlines {
                data.bytes(scanline);
            }

            (PsdChannelCompression::RleCompressed, data.into_bytes())
        }
        _ => (PsdChannelCompression::RawData, plane.to_vec()),
    }
}
