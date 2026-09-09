//! A small big endian byte writer used to serialize PSD files.

/// A growable buffer that knows how to append the big endian primitives and
/// string encodings that a PSD file is made of.
pub(crate) struct ByteWriter {
    buf: Vec<u8>,
}

impl ByteWriter {
    pub fn new() -> ByteWriter {
        ByteWriter { buf: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> ByteWriter {
        ByteWriter {
            buf: Vec::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i16(&mut self, value: i16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn zeros(&mut self, count: usize) {
        self.buf.resize(self.buf.len() + count, 0);
    }

    /// Write a u32 length marker followed by the bytes that it describes.
    pub fn length_prefixed(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.bytes(bytes);
    }

    /// Append zeros until the buffer's length is a multiple of `multiple`.
    pub fn pad_to_multiple_of(&mut self, multiple: usize) {
        let remainder = self.buf.len() % multiple;
        if remainder != 0 {
            self.zeros(multiple - remainder);
        }
    }

    /// Write a Pascal string - a single length byte followed by that many bytes.
    ///
    /// The length byte and the string are together padded with zeros until they
    /// are a multiple of `multiple` bytes long.
    ///
    /// Names that do not fit in the single length byte are truncated on a UTF-8
    /// character boundary. Photoshop does the same thing - the full name is
    /// preserved in the layer's `luni` (unicode name) block.
    pub fn pascal_string(&mut self, string: &str, multiple: usize) {
        let string = truncate_on_char_boundary(string, u8::MAX as usize);

        let start = self.buf.len();

        self.u8(string.len() as u8);
        self.bytes(string.as_bytes());

        let written = self.buf.len() - start;
        let remainder = written % multiple;
        if remainder != 0 {
            self.zeros(multiple - remainder);
        }
    }

    /// Write a PSD "Unicode string" - a u32 count of UTF-16 code units followed
    /// by those code units, each written as a big endian u16.
    pub fn unicode_string(&mut self, string: &str) {
        let utf16: Vec<u16> = string.encode_utf16().collect();

        self.u32(utf16.len() as u32);
        for code_unit in utf16 {
            self.u16(code_unit);
        }
    }
}

/// Shorten `string` so that it is at most `max_bytes` long without splitting a
/// multi byte UTF-8 character in half.
fn truncate_on_char_boundary(string: &str, max_bytes: usize) -> &str {
    if string.len() <= max_bytes {
        return string;
    }

    let mut end = max_bytes;
    while end > 0 && !string.is_char_boundary(end) {
        end -= 1;
    }

    &string[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_string_is_padded() {
        let mut writer = ByteWriter::new();
        writer.pascal_string("abc", 4);

        assert_eq!(writer.as_slice(), &[3, b'a', b'b', b'c']);

        let mut writer = ByteWriter::new();
        writer.pascal_string("ab", 4);

        assert_eq!(writer.as_slice(), &[2, b'a', b'b', 0]);
    }

    #[test]
    fn pascal_string_truncates_on_char_boundary() {
        let name = "é".repeat(200);

        let mut writer = ByteWriter::new();
        writer.pascal_string(&name, 4);

        // 255 bytes would split the 128th two byte character in half, so we
        // stop at 254 bytes.
        assert_eq!(writer.as_slice()[0], 254);
    }

    #[test]
    fn unicode_string_is_utf16_big_endian() {
        let mut writer = ByteWriter::new();
        writer.unicode_string("hi");

        assert_eq!(writer.as_slice(), &[0, 0, 0, 2, 0, b'h', 0, b'i']);
    }
}
