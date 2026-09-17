//! RFC-0036 interchange extension envelope.
//!
//! Every extensible file/message wraps its append-only tail in a sequence of
//! `(u16 extension_id, u16 flags, u32 length, payload)` records sorted by
//! `extension_id`. ID 0 and 1..1023 are core, 1024..49151 registered vendor,
//! and 49152..65535 private ids that MUST NOT appear in a canonical artifact.
//! Unknown required extensions fail; unknown optional ones may be skipped after
//! length validation.

use crate::sha256::digest;
use crate::wire::{Reader, Writer};
use pwe_api::{Error, Hash256, Result, Status};

fn error(status: Status, detail: u32, offset: usize) -> Error {
    Error {
        status,
        detail,
        byte_offset: offset as u64,
    }
}

pub const EXT_FLAG_OPTIONAL: u16 = 1;
pub const EXT_FLAG_REQUIRED: u16 = 2;

const ID_PRIVATE_MIN: u16 = 49152;
/// Core compression extension id (RFC-0036). Payload is a
/// `CompressionMetadata` record.
pub const EXT_COMPRESSION: u16 = 2;
/// Compression algorithms (RFC-0036 declares an algorithm id).
pub const ALGO_NONE: u8 = 0;
pub const ALGO_DEFLATE: u8 = 1;

/// Metadata a compression extension carries: the algorithm, the uncompressed
/// byte length, and a SHA-256 of the uncompressed payload. Decompression is
/// bounded by the advertised `max_document_bytes` (RFC-0034).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressionMetadata {
    pub algorithm: u8,
    pub uncompressed_length: u64,
    pub uncompressed_sha256: Hash256,
}

impl CompressionMetadata {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Writer::new();
        out.u8(self.algorithm)?;
        out.u64(self.uncompressed_length)?;
        out.bytes_raw(&self.uncompressed_sha256.0)?;
        Ok(out.finish())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        let algorithm = input.fixed(1)?[0];
        let uncompressed_length = input.u64()?;
        let uncompressed_sha256 = Hash256(input.fixed(32)?.try_into().unwrap());
        input.finish()?;
        if uncompressed_length as usize > crate::wire::MAX_DOCUMENT_BYTES {
            return Err(error(Status::Limit, 1, 0));
        }
        Ok(CompressionMetadata {
            algorithm,
            uncompressed_length,
            uncompressed_sha256,
        })
    }
    /// Convenience: build metadata for an uncompressed payload.
    pub fn for_payload(algorithm: u8, payload: &[u8]) -> Self {
        CompressionMetadata {
            algorithm,
            uncompressed_length: payload.len() as u64,
            uncompressed_sha256: digest(payload),
        }
    }
}

/// A single extension payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Extension {
    pub id: u16,
    pub flags: u16,
    pub payload: Vec<u8>,
}

/// A set of extensions forming the tail of a canonical artifact.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtensionEnvelope {
    pub extensions: Vec<Extension>,
}

impl Extension {
    fn is_private(&self) -> bool {
        self.id >= ID_PRIVATE_MIN
    }
}

impl ExtensionEnvelope {
    /// Encodes the extensions in canonical form: sorted by id, no duplicates,
    /// and private ids are rejected (they are forbidden in canonical output).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut sorted = self.extensions.clone();
        sorted.sort_by_key(|e| e.id);
        if sorted.windows(2).any(|p| p[0].id == p[1].id) {
            return Err(error(Status::Invalid, 1, 0));
        }
        if let Some(private) = sorted.iter().find(|e| e.is_private()) {
            return Err(error(Status::Invalid, 2, private.id as usize));
        }
        let mut out = Writer::new();
        for extension in &sorted {
            if extension.flags & !(EXT_FLAG_OPTIONAL | EXT_FLAG_REQUIRED) != 0 {
                return Err(error(Status::Invalid, 3, extension.id as usize));
            }
            out.u16(extension.id)?;
            out.u16(extension.flags)?;
            out.bytes(&extension.payload)?;
        }
        Ok(out.finish())
    }

    /// Decodes the extension envelope. `known` is the set of ids the reader
    /// understands; a required extension outside `known` fails, an optional one
    /// is retained for the caller to skip after length validation.
    pub fn decode(bytes: &[u8], known: &[u16]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        let mut extensions = Vec::new();
        let mut previous_id: Option<u16> = None;
        while !input.is_empty() {
            let id = input.u16()?;
            let flags = input.u16()?;
            let payload = input.bytes()?.to_vec();
            if flags & !(EXT_FLAG_OPTIONAL | EXT_FLAG_REQUIRED) != 0 {
                return Err(error(Status::Invalid, 3, id as usize));
            }
            if previous_id.is_some_and(|prev| prev >= id) {
                return Err(error(Status::Invalid, 4, id as usize));
            }
            previous_id = Some(id);
            if id >= ID_PRIVATE_MIN {
                // Private ids must not appear in canonical artifacts.
                return Err(error(Status::Invalid, 5, id as usize));
            }
            let required = flags & EXT_FLAG_REQUIRED != 0;
            if required && !known.contains(&id) {
                return Err(error(Status::SchemaUnsupported, 1, id as usize));
            }
            extensions.push(Extension { id, flags, payload });
        }
        Ok(ExtensionEnvelope { extensions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(id: u16, flags: u16, payload: &[u8]) -> Extension {
        Extension {
            id,
            flags,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn extension_envelope_round_trips_sorted() {
        let envelope = ExtensionEnvelope {
            extensions: vec![
                ext(2, EXT_FLAG_OPTIONAL, b"two"),
                ext(1, EXT_FLAG_REQUIRED, b"one"),
            ],
        };
        let bytes = envelope.encode().unwrap();
        let decoded = ExtensionEnvelope::decode(&bytes, &[1]).unwrap();
        assert_eq!(decoded.extensions[0].id, 1);
        assert_eq!(decoded.extensions[1].id, 2);
        assert_eq!(decoded.extensions[0].payload, b"one");
    }

    #[test]
    fn extension_rejects_duplicate_and_private_ids() {
        let dup = ExtensionEnvelope {
            extensions: vec![ext(1, 0, b"a"), ext(1, 0, b"b")],
        };
        assert_eq!(dup.encode().unwrap_err().status, Status::Invalid);
        let private = ExtensionEnvelope {
            extensions: vec![ext(50000, 0, b"x")],
        };
        assert_eq!(private.encode().unwrap_err().status, Status::Invalid);
    }

    #[test]
    fn extension_required_unknown_fails_optional_skipped() {
        let required_unknown = ExtensionEnvelope {
            extensions: vec![ext(9, EXT_FLAG_REQUIRED, b"x")],
        };
        let bytes = required_unknown.encode().unwrap();
        assert_eq!(
            ExtensionEnvelope::decode(&bytes, &[1]).unwrap_err().status,
            Status::SchemaUnsupported
        );
        let optional_unknown = ExtensionEnvelope {
            extensions: vec![ext(9, EXT_FLAG_OPTIONAL, b"x")],
        };
        let bytes = optional_unknown.encode().unwrap();
        assert!(ExtensionEnvelope::decode(&bytes, &[1]).is_ok());
    }

    #[test]
    fn compression_metadata_round_trips_and_is_bounded() {
        let meta = CompressionMetadata::for_payload(ALGO_DEFLATE, b"uncompressed data");
        let bytes = meta.encode().unwrap();
        let decoded = CompressionMetadata::decode(&bytes).unwrap();
        assert_eq!(decoded, meta);
        assert_eq!(decoded.algorithm, ALGO_DEFLATE);
        assert_eq!(
            decoded.uncompressed_length,
            b"uncompressed data".len() as u64
        );
        // A declared uncompressed length past the document limit is rejected.
        let oversized = CompressionMetadata {
            algorithm: ALGO_DEFLATE,
            uncompressed_length: crate::wire::MAX_DOCUMENT_BYTES as u64 + 1,
            uncompressed_sha256: Hash256([0; 32]),
        };
        assert_eq!(
            CompressionMetadata::decode(&oversized.encode().unwrap())
                .unwrap_err()
                .status,
            Status::Limit
        );
    }

    #[test]
    fn extension_rejects_out_of_order_ids() {
        let mut bytes = ExtensionEnvelope {
            extensions: vec![ext(2, 0, b"a"), ext(1, 0, b"b")],
        }
        .encode()
        .unwrap();
        // Re-encode already sorts, so build a raw out-of-order byte stream.
        bytes.clear();
        let mut out = crate::wire::Writer::new();
        out.u16(2).unwrap();
        out.u16(0).unwrap();
        out.bytes(b"a").unwrap();
        out.u16(1).unwrap();
        out.u16(0).unwrap();
        out.bytes(b"b").unwrap();
        bytes.extend(out.finish());
        assert_eq!(
            ExtensionEnvelope::decode(&bytes, &[1, 2])
                .unwrap_err()
                .status,
            Status::Invalid
        );
    }
}
