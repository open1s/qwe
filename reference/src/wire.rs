//! Bounded canonical byte primitives shared by WIR, snapshots, and EIR.

use pwe_api::{Error, Result, Status};

/// Bound derived from the RFC-0034 advertised default, so the enforced limit is
/// a single source of truth with `pwe_api::limits::DEFAULT_LIMITS`.
pub const MAX_DOCUMENT_BYTES: usize = pwe_api::limits::DEFAULT_LIMITS.max_document_bytes as usize;

pub fn error(status: Status, offset: usize) -> Error {
    Error {
        status,
        detail: 0,
        byte_offset: offset as u64,
    }
}

/// CRC-32C (Castagnoli), used for WIR and EIR section integrity.
pub fn crc32c(bytes: &[u8]) -> u32 {
    fn table() -> &'static [u32; 256] {
        static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
        T.get_or_init(|| {
            let mut t = [0u32; 256];
            for (i, entry) in t.iter_mut().enumerate() {
                let mut crc = i as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0x82F6_3B78
                    } else {
                        crc >> 1
                    };
                }
                *entry = crc;
            }
            t
        })
    }
    let mut crc = !0u32;
    for &b in bytes {
        crc = (crc >> 8) ^ table()[((crc ^ b as u32) & 0xff) as usize];
    }
    !crc
}

#[derive(Default, Debug)]
pub struct Writer {
    bytes: Vec<u8>,
}
impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reserve(&self, additional: usize) -> Result<()> {
        if self
            .bytes
            .len()
            .checked_add(additional)
            .filter(|n| *n <= MAX_DOCUMENT_BYTES)
            .is_none()
        {
            return Err(error(Status::Limit, self.bytes.len()));
        }
        Ok(())
    }
    pub fn u16(&mut self, value: u16) -> Result<()> {
        self.reserve(2)?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    pub fn u32(&mut self, value: u32) -> Result<()> {
        self.reserve(4)?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    pub fn u64(&mut self, value: u64) -> Result<()> {
        self.reserve(8)?;
        self.bytes.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    pub fn u8(&mut self, value: u8) -> Result<()> {
        self.reserve(1)?;
        self.bytes.push(value);
        Ok(())
    }
    /// Appends fixed-format bytes without a length prefix.
    pub fn bytes_raw(&mut self, value: &[u8]) -> Result<()> {
        self.reserve(value.len())?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }
    pub fn bytes(&mut self, value: &[u8]) -> Result<()> {
        let length =
            u32::try_from(value.len()).map_err(|_| error(Status::Limit, self.bytes.len()))?;
        self.reserve(4 + value.len())?;
        self.u32(length)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            Err(error(Status::Limit, 0))
        } else {
            Ok(Self { bytes, cursor: 0 })
        }
    }
    pub fn offset(&self) -> usize {
        self.cursor
    }
    pub fn is_empty(&self) -> bool {
        self.cursor == self.bytes.len()
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(error(Status::Invalid, self.cursor))?;
        let value = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(value)
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let length = usize::try_from(self.u32()?).map_err(|_| error(Status::Limit, self.cursor))?;
        self.take(length)
    }
    pub fn fixed(&mut self, length: usize) -> Result<&'a [u8]> {
        self.take(length)
    }
    pub fn finish(self) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(error(Status::Invalid, self.cursor))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_values_round_trip() {
        let mut out = Writer::new();
        out.u16(0x0102).unwrap();
        out.u32(7).unwrap();
        out.u64(9).unwrap();
        out.bytes(b"pwe").unwrap();
        let bytes = out.finish();
        assert_eq!(&bytes[..2], &[2, 1]);
        let mut input = Reader::new(&bytes).unwrap();
        assert_eq!(input.u16().unwrap(), 0x0102);
        assert_eq!(input.u32().unwrap(), 7);
        assert_eq!(input.u64().unwrap(), 9);
        assert_eq!(input.bytes().unwrap(), b"pwe");
        input.finish().unwrap();
        // Re-encoding the decoded values is byte-identical.
        let mut again = Writer::new();
        again.u16(0x0102).unwrap();
        again.u32(7).unwrap();
        again.u64(9).unwrap();
        again.bytes(b"pwe").unwrap();
        assert_eq!(again.finish(), bytes);
    }
    #[test]
    fn canonical_malformed_lengths_fail_without_allocation() {
        // Declared length far beyond the document must fail without reading or allocating.
        let declared = u32::MAX.to_le_bytes();
        let mut input = Reader::new(&declared).unwrap();
        let err = input.bytes().unwrap_err();
        assert_eq!(err.status, Status::Invalid);
        assert_eq!(err.byte_offset, 4);
        // A huge declared length with most of a record missing is still bounded.
        let mut input = Reader::new(&[0xff, 0xff, 0xff, 0x7f, 0]).unwrap();
        assert_eq!(input.bytes().unwrap_err().status, Status::Invalid);
        // Zero-length byte strings are legal.
        let mut out = Writer::new();
        out.bytes(&[]).unwrap();
        let empty_bytes = out.finish();
        let mut input = Reader::new(&empty_bytes).unwrap();
        assert_eq!(input.bytes().unwrap(), &[]);
        input.finish().unwrap();
        // Oversized input documents are rejected up front.
        let oversized = vec![0u8; MAX_DOCUMENT_BYTES + 1];
        assert_eq!(
            Reader::new(&oversized).err().map(|e| e.status),
            Some(Status::Limit)
        );
    }
    #[test]
    fn truncation_and_trailing_bytes_fail() {
        assert_eq!(
            Reader::new(&[1]).unwrap().u16().unwrap_err().status,
            Status::Invalid
        );
        let mut input = Reader::new(&[0, 0, 7]).unwrap();
        input.u16().unwrap();
        assert_eq!(input.finish().unwrap_err().status, Status::Invalid);
    }
}
