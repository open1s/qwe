//! RFC-0020 bounded WIR binary model: envelope, directory, and REQUIRED
//! SCHEMA / ENTITIES / COMPONENTS sections with conditional validation order.

use crate::schema::{Schema, SchemaRegistry};
use crate::wire::{crc32c, Reader, Writer, MAX_DOCUMENT_BYTES};
use pwe_api::{ComponentTypeId, EntityId, Error, Hash256, Result, Status, WorldId, WorldVersion};
use std::collections::BTreeMap;

pub const WIR_MAGIC: [u8; 8] = *b"PWEWIR2\0";
pub const WIR_MAJOR: u16 = 2;
pub const WIR_MINOR: u16 = 0;

pub const FLAG_INITIAL_STATE: u32 = 1;
pub const FLAG_DEBUG_NAMES: u32 = 2;

const DIR_FLAG_OPTIONAL: u16 = 1;
const DIR_FLAG_AUTHORITATIVE: u16 = 2;
const DIR_FLAG_COMPRESSED: u16 = 4;
const DIR_FLAG_MASK: u16 = DIR_FLAG_OPTIONAL | DIR_FLAG_AUTHORITATIVE | DIR_FLAG_COMPRESSED;

const KIND_SCHEMA: u16 = 1;
const KIND_ENTITIES: u16 = 2;
const KIND_COMPONENTS: u16 = 3;
const KIND_RESOURCES: u16 = 4;
const KIND_SPATIAL: u16 = 5;
const KIND_SYSTEMS: u16 = 6;
const KIND_EVENTS: u16 = 7;
const KIND_TIMELINES: u16 = 8;
const KIND_EXTENSIONS: u16 = 9;
/// Highest RFC-0020 section kind. Kinds 0 or > this are unknown.
const KIND_MAX: u16 = KIND_EXTENSIONS;

const MAX_SECTIONS: u32 = pwe_api::limits::DEFAULT_LIMITS.max_sections;
const MAX_RECORDS: u32 = pwe_api::limits::DEFAULT_LIMITS.max_records;

const HEADER_LEN: usize = 88;
const DIR_ENTRY_LEN: usize = 32;

fn error(status: Status, detail: u32, offset: usize) -> Error {
    Error {
        status,
        detail,
        byte_offset: offset as u64,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityRecord {
    pub id: EntityId,
    pub generation: u32,
    pub flags: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentRecord {
    pub type_id: ComponentTypeId,
    pub entity: EntityId,
    pub generation: u32,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WirDocument {
    pub flags: u32,
    pub world_id: WorldId,
    pub world_version: WorldVersion,
    pub sim_time_ns: i64,
    pub schemas: Vec<Schema>,
    pub entities: Vec<EntityRecord>,
    pub components: Vec<ComponentRecord>,
    /// RFC-0020 sections 4–8 (RESOURCES, SPATIAL, SYSTEMS, EVENTS, TIMELINES):
    /// generic variable-length record containers, decoded and validated for
    /// framing. Each record is an opaque bounded byte string.
    pub resources: Vec<Vec<u8>>,
    pub spatial: Vec<Vec<u8>>,
    pub systems: Vec<Vec<u8>>,
    pub events: Vec<Vec<u8>>,
    pub timelines: Vec<Vec<u8>>,
}

#[derive(Clone)]
struct Section {
    kind: u16,
    flags: u16,
    body: Vec<u8>,
}

fn u128(v: u128) -> Result<()> {
    if v == 0 {
        return Err(error(Status::Invalid, 20, 0));
    }
    Ok(())
}

impl WirDocument {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.flags & !(FLAG_INITIAL_STATE | FLAG_DEBUG_NAMES) != 0 {
            return Err(error(Status::Invalid, 1, 0));
        }
        u128(self.world_id.0)?;
        let registry = self.build_registry()?;
        let schema_set_hash = registry.schema_set_hash();

        let entity_ids: BTreeMap<(EntityId, u32), ()> = self
            .entities
            .iter()
            .map(|e| ((e.id, e.generation), ()))
            .collect();
        for component in &self.components {
            if registry.get(component.type_id).is_none() {
                return Err(error(Status::SchemaHash, 2, 0));
            }
            if !entity_ids.contains_key(&(component.entity, component.generation)) {
                return Err(error(Status::Invalid, 14, 0));
            }
        }

        let schemas = encode_schema_section(self.schemas.clone())?;
        let entities = encode_entities_section(self.entities.clone())?;
        let components = encode_components_section(self.components.clone())?;

        let mut sections = vec![
            Section {
                kind: KIND_SCHEMA,
                flags: 0,
                body: schemas,
            },
            Section {
                kind: KIND_ENTITIES,
                flags: 0,
                body: entities,
            },
            Section {
                kind: KIND_COMPONENTS,
                flags: 0,
                body: components,
            },
        ];
        // RFC-0020 sections 4–8: emit each only when non-empty, in ascending
        // kind order, so documents that do not use them are unchanged.
        for (kind, records) in [
            (KIND_RESOURCES, &self.resources),
            (KIND_SPATIAL, &self.spatial),
            (KIND_SYSTEMS, &self.systems),
            (KIND_EVENTS, &self.events),
            (KIND_TIMELINES, &self.timelines),
        ] {
            if !records.is_empty() {
                sections.push(Section {
                    kind,
                    flags: 0,
                    body: encode_record_section(records)?,
                });
            }
        }

        // Layout sections after the directory with 8-byte alignment.
        let mut offsets: Vec<usize> = Vec::with_capacity(sections.len());
        let mut cursor = HEADER_LEN + sections.len() * DIR_ENTRY_LEN;
        for section in &sections {
            cursor =
                cursor
                    .checked_add(7)
                    .map(|n| n & !7)
                    .ok_or(error(Status::Limit, 2, cursor))?;
            offsets.push(cursor);
            cursor =
                cursor
                    .checked_add(section.body.len())
                    .ok_or(error(Status::Limit, 3, cursor))?;
        }

        let mut out = Writer::new();
        out.bytes_raw(&WIR_MAGIC)?;
        out.u16(WIR_MAJOR)?;
        out.u16(WIR_MINOR)?;
        out.u32(self.flags)?;
        out.bytes_raw(&schema_set_hash.0)?;
        out.bytes_raw(&self.world_id.0.to_le_bytes())?;
        out.u64(self.world_version.0)?;
        out.u64(self.sim_time_ns as u64)?;
        out.u32(sections.len() as u32)?;

        // header_crc32c over bytes 0..79 (everything up to but not including
        // section_count and the CRC itself).
        let partial = out.finish();
        let header_crc = crc32c(&partial[..80]);
        let mut out = Writer::new();
        out.bytes_raw(&partial)?;
        out.u32(header_crc)?;

        for (section, offset) in sections.iter().zip(offsets.iter()) {
            let crc = crc32c(&section.body);
            out.u16(section.kind)?;
            out.u16(section.flags)?;
            out.u32(0)?;
            out.u64(*offset as u64)?;
            out.u64(section.body.len() as u64)?;
            out.u32(crc)?;
            out.u32(0)?;
        }
        let header_and_dir = out.finish();
        let mut out = Writer::new();
        out.bytes_raw(&header_and_dir)?;
        let mut written = header_and_dir.len();
        for (section, offset) in sections.iter().zip(offsets.iter()) {
            let padding = *offset - written;
            out.bytes_raw(&vec![0u8; padding])?;
            out.bytes_raw(&section.body)?;
            written = *offset + section.body.len();
        }
        Ok(out.finish())
    }

    fn build_registry(&self) -> Result<SchemaRegistry> {
        let mut registry = SchemaRegistry::default();
        for schema in &self.schemas {
            registry.register(schema.clone())?;
        }
        Ok(registry)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;

        // 1. magic / version / flags.
        if input.fixed(8)? != WIR_MAGIC {
            return Err(error(Status::Invalid, 4, 0));
        }
        if input.u16()? != WIR_MAJOR || input.u16()? != WIR_MINOR {
            return Err(error(Status::Invalid, 5, 0));
        }
        let flags = input.u32()?;
        if flags & !(FLAG_INITIAL_STATE | FLAG_DEBUG_NAMES) != 0 {
            return Err(error(Status::Invalid, 6, 0));
        }

        // 2. header CRC over bytes 0..79.
        let header_crc = crc32c(&bytes[..80.min(bytes.len())]);
        if header_crc
            != u32::from_le_bytes(
                bytes[84..88]
                    .try_into()
                    .map_err(|_| error(Status::Invalid, 7, 84))?,
            )
        {
            return Err(error(Status::Invalid, 8, 84));
        }

        let schema_set_hash = Hash256(input.fixed(32)?.try_into().unwrap());
        let world_id = WorldId(u128::from_le_bytes(input.fixed(16)?.try_into().unwrap()));
        let world_version = WorldVersion(input.u64()?);
        let sim_time_ns = input.u64()? as i64;

        // 3. limits.
        let section_count = input.u32()?;
        let _stored_header_crc = input.u32()?;
        if section_count > MAX_SECTIONS {
            return Err(error(Status::Limit, 1, 80));
        }

        // 4. directory.
        let mut directory = Vec::with_capacity(section_count as usize);
        for _ in 0..section_count {
            let kind = input.u16()?;
            let dir_flags = input.u16()?;
            let reserved0 = input.u32()?;
            let offset = input.u64()?;
            let length = input.u64()?;
            let section_crc = input.u32()?;
            let reserved1 = input.u32()?;
            if reserved0 != 0 || reserved1 != 0 {
                return Err(error(Status::Invalid, 9, input.offset()));
            }
            if dir_flags & !DIR_FLAG_MASK != 0 {
                return Err(error(Status::Invalid, 17, input.offset()));
            }
            if dir_flags & DIR_FLAG_COMPRESSED != 0 {
                return Err(error(Status::SchemaUnsupported, 1, input.offset()));
            }
            directory.push((
                kind,
                dir_flags,
                offset as usize,
                length as usize,
                section_crc,
            ));
        }

        // Directory must be non-overlapping, 8-aligned, ascending, after bytes.
        let dir_end = input.offset();
        let mut previous_end = dir_end;
        let mut kinds_seen = BTreeMap::new();
        for &(kind, _, offset, length, section_crc) in &directory {
            // Unknown (non-optional) kinds fail; a known kind occurs once.
            if kind == 0 || kind > KIND_MAX {
                return Err(error(Status::Invalid, 15, offset));
            }
            if kinds_seen.contains_key(&kind) {
                return Err(error(Status::Invalid, 16, offset));
            }
            if offset < dir_end || offset % 8 != 0 || offset < previous_end {
                return Err(error(Status::Invalid, 10, offset));
            }
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or(error(Status::Invalid, 11, offset))?;
            if end > MAX_DOCUMENT_BYTES {
                return Err(error(Status::Limit, 2, offset));
            }
            // 5. section CRC.
            if crc32c(&bytes[offset..end]) != section_crc {
                return Err(error(Status::Invalid, 12, offset));
            }
            previous_end = end;
            kinds_seen.insert(kind, (offset, length));
        }

        // Full consumption: the last section must reach the end of the document;
        // any trailing bytes after the final section are invalid.
        if previous_end != bytes.len() {
            return Err(error(Status::Invalid, 18, previous_end));
        }

        // 6. uniqueness + REQUIRED sections.
        for required in [KIND_SCHEMA, KIND_ENTITIES, KIND_COMPONENTS] {
            if !kinds_seen.contains_key(&required) {
                return Err(error(Status::Invalid, 13, 0));
            }
        }

        let (schema_off, schema_len) = *kinds_seen.get(&KIND_SCHEMA).unwrap();
        let (entities_off, entities_len) = *kinds_seen.get(&KIND_ENTITIES).unwrap();
        let (components_off, components_len) = *kinds_seen.get(&KIND_COMPONENTS).unwrap();

        let schemas = decode_schema_section(&bytes[schema_off..schema_off + schema_len])?;
        let entities = decode_entities_section(&bytes[entities_off..entities_off + entities_len])?;
        let components =
            decode_components_section(&bytes[components_off..components_off + components_len])?;

        // RFC-0020 sections 4–8: decode (when present) as generic record
        // containers, validating framing and bounds.
        let mut record_sections = BTreeMap::new();
        for (kind, field) in [
            (KIND_RESOURCES, b"resources".as_slice()),
            (KIND_SPATIAL, b"spatial".as_slice()),
            (KIND_SYSTEMS, b"systems".as_slice()),
            (KIND_EVENTS, b"events".as_slice()),
            (KIND_TIMELINES, b"timelines".as_slice()),
        ] {
            if let Some(&(off, len)) = kinds_seen.get(&kind) {
                record_sections.insert(field, decode_record_section(&bytes[off..off + len])?);
            }
        }
        let get =
            |key: &[u8]| -> Vec<Vec<u8>> { record_sections.get(key).cloned().unwrap_or_default() };
        let resources = get(b"resources");
        let spatial = get(b"spatial");
        let systems = get(b"systems");
        let events = get(b"events");
        let timelines = get(b"timelines");

        // 8. schema closure: verify schema_set_hash and resolve references.
        let mut registry = SchemaRegistry::default();
        for schema in &schemas {
            registry.register(schema.clone())?;
        }
        if registry.schema_set_hash() != schema_set_hash {
            return Err(error(Status::SchemaHash, 1, 0));
        }

        // 9. references: every component type must be a known schema, and every
        // component entity must reference a declared entity.
        let entity_ids: BTreeMap<(EntityId, u32), ()> = entities
            .iter()
            .map(|e| ((e.id, e.generation), ()))
            .collect();
        for component in &components {
            if registry.get(component.type_id).is_none() {
                return Err(error(Status::SchemaHash, 2, 0));
            }
            if !entity_ids.contains_key(&(component.entity, component.generation)) {
                return Err(error(Status::Invalid, 14, 0));
            }
        }

        Ok(Self {
            flags,
            world_id,
            world_version,
            sim_time_ns,
            schemas,
            entities,
            components,
            resources,
            spatial,
            systems,
            events,
            timelines,
        })
    }
}

fn encode_schema_section(schemas: Vec<Schema>) -> Result<Vec<u8>> {
    let mut sorted = schemas;
    sorted.sort_by(|a, b| {
        a.identity
            .canonical_bytes()
            .ok()
            .cmp(&b.identity.canonical_bytes().ok())
    });
    let mut out = Writer::new();
    out.u32(u32::try_from(sorted.len()).map_err(|_| error(Status::Limit, 4, 0))?)?;
    for schema in &sorted {
        out.bytes(&schema.canonical_bytes()?)?;
    }
    Ok(out.finish())
}

fn decode_schema_section(bytes: &[u8]) -> Result<Vec<Schema>> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()?;
    if count > MAX_RECORDS {
        return Err(error(Status::Limit, 3, 0));
    }
    let mut schemas = Vec::new();
    for _ in 0..count {
        let canonical = input.bytes()?;
        let decoded = Schema::decode(canonical)?;
        schemas.push(decoded);
    }
    input.finish()?;
    Ok(schemas)
}

fn encode_entities_section(entities: Vec<EntityRecord>) -> Result<Vec<u8>> {
    let mut sorted = entities;
    sorted.sort_by_key(|a| (a.id, a.generation));
    if sorted
        .windows(2)
        .any(|pair| pair[0].id == pair[1].id && pair[0].generation == pair[1].generation)
    {
        return Err(error(Status::Invalid, 15, 0));
    }
    let mut out = Writer::new();
    out.u32(u32::try_from(sorted.len()).map_err(|_| error(Status::Limit, 5, 0))?)?;
    for entity in &sorted {
        u128(entity.id.0)?;
        if entity.generation == 0 {
            return Err(error(Status::Invalid, 16, 0));
        }
        out.bytes_raw(&entity.id.0.to_le_bytes())?;
        out.u32(entity.generation)?;
        out.u32(entity.flags)?;
    }
    Ok(out.finish())
}

fn decode_entities_section(bytes: &[u8]) -> Result<Vec<EntityRecord>> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()?;
    if count > MAX_RECORDS {
        return Err(error(Status::Limit, 4, 0));
    }
    let mut entities = Vec::new();
    let mut previous = None::<(EntityId, u32)>;
    for _ in 0..count {
        let id = EntityId(u128::from_le_bytes(input.fixed(16)?.try_into().unwrap()));
        let generation = input.u32()?;
        let flags = input.u32()?;
        if id.0 == 0 || generation == 0 {
            return Err(error(Status::Invalid, 17, input.offset()));
        }
        let key = (id, generation);
        if previous.is_some_and(|prev| prev >= key) {
            return Err(error(Status::Invalid, 18, input.offset()));
        }
        previous = Some(key);
        entities.push(EntityRecord {
            id,
            generation,
            flags,
        });
    }
    if !input.is_empty() {
        return Err(error(Status::Invalid, 19, input.offset()));
    }
    Ok(entities)
}

fn encode_components_section(components: Vec<ComponentRecord>) -> Result<Vec<u8>> {
    let mut sorted = components;
    sorted.sort_by(|a, b| {
        (a.type_id, a.entity, a.generation).cmp(&(b.type_id, b.entity, b.generation))
    });
    if sorted.windows(2).any(|pair| {
        pair[0].type_id == pair[1].type_id
            && pair[0].entity == pair[1].entity
            && pair[0].generation == pair[1].generation
    }) {
        return Err(error(Status::Invalid, 21, 0));
    }
    let mut out = Writer::new();
    out.u32(u32::try_from(sorted.len()).map_err(|_| error(Status::Limit, 6, 0))?)?;
    for component in &sorted {
        let mut record = Writer::new();
        record.bytes_raw(&component.type_id.0)?;
        u128(component.entity.0)?;
        record.bytes_raw(&component.entity.0.to_le_bytes())?;
        record.u32(component.generation)?;
        record.bytes(&component.value)?;
        let record_bytes = record.finish();
        out.bytes(&record_bytes)?;
    }
    Ok(out.finish())
}

fn decode_components_section(bytes: &[u8]) -> Result<Vec<ComponentRecord>> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()?;
    if count > MAX_RECORDS {
        return Err(error(Status::Limit, 5, 0));
    }
    let mut components = Vec::new();
    let mut previous = None::<(ComponentTypeId, EntityId, u32)>;
    for _ in 0..count {
        let record_bytes = input.bytes()?;
        let mut record = Reader::new(record_bytes)?;
        let type_id = ComponentTypeId(record.fixed(16)?.try_into().unwrap());
        let entity = EntityId(u128::from_le_bytes(record.fixed(16)?.try_into().unwrap()));
        let generation = record.u32()?;
        let value = record.bytes()?.to_vec();
        record.finish()?;
        if entity.0 == 0 || generation == 0 {
            return Err(error(Status::Invalid, 22, input.offset()));
        }
        let key = (type_id, entity, generation);
        if previous.is_some_and(|prev| prev >= key) {
            return Err(error(Status::Invalid, 23, input.offset()));
        }
        previous = Some(key);
        components.push(ComponentRecord {
            type_id,
            entity,
            generation,
            value,
        });
    }
    if !input.is_empty() {
        return Err(error(Status::Invalid, 24, input.offset()));
    }
    Ok(components)
}

/// RFC-0020 sections 4–8 are generic variable-length record containers:
/// `u32 record_count`, then each record is `u32 byte_len` + bytes, consumed
/// exactly. Encodes the given opaque records into a section body.
fn encode_record_section(records: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut out = Writer::new();
    out.u32(u32::try_from(records.len()).map_err(|_| error(Status::Limit, 7, 0))?)?;
    for record in records {
        out.bytes(record)?;
    }
    Ok(out.finish())
}

/// Decodes a generic record section, validating record framing, bounds, and
/// exact consumption (RFC-0020 section framing).
fn decode_record_section(bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut input = Reader::new(bytes)?;
    let count = input.u32()?;
    if count > MAX_RECORDS {
        return Err(error(Status::Limit, 6, 0));
    }
    let mut records = Vec::with_capacity(count as usize);
    for _ in 0..count {
        records.push(input.bytes()?.to_vec());
    }
    input.finish()?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ComponentIdentity, Field, FieldType};

    fn schema(name: &str) -> Schema {
        Schema {
            identity: ComponentIdentity {
                namespace: "pwe.physics".into(),
                stable_name: name.into(),
                major_version: 1,
            },
            fields: vec![
                Field {
                    id: 1,
                    name: "id".into(),
                    ty: FieldType::U64,
                    flags: 1,
                },
                Field {
                    id: 2,
                    name: "mass".into(),
                    ty: FieldType::F32,
                    flags: 1,
                },
            ],
        }
    }

    fn doc() -> WirDocument {
        let schemas = vec![schema("body")];
        let body_id = schemas[0].identity.type_id().unwrap();
        WirDocument {
            flags: FLAG_INITIAL_STATE,
            world_id: WorldId(1),
            world_version: WorldVersion(0),
            sim_time_ns: 0,
            schemas,
            entities: vec![EntityRecord {
                id: EntityId(1),
                generation: 1,
                flags: 0,
            }],
            components: vec![ComponentRecord {
                type_id: body_id,
                entity: EntityId(1),
                generation: 1,
                value: vec![1, 2, 3, 4],
            }],
            resources: Vec::new(),
            spatial: Vec::new(),
            systems: Vec::new(),
            events: Vec::new(),
            timelines: Vec::new(),
        }
    }

    #[test]
    fn wir_canonical_round_trip_is_byte_identical() {
        let bytes = doc().encode().unwrap();
        let decoded = WirDocument::decode(&bytes).unwrap();
        let reencoded = decoded.encode().unwrap();
        assert_eq!(reencoded, bytes);
        assert_eq!(decoded.components.len(), 1);
    }

    #[test]
    fn wir_sections_4_through_8_decode_and_round_trip() {
        let mut d = doc();
        d.resources = vec![vec![1, 2, 3]];
        d.spatial = vec![vec![0xAA], vec![0xBB, 0xCC, 0xDD, 0xEE]];
        d.systems = vec![vec![9, 8, 7]];
        d.events = vec![vec![42]];
        d.timelines = vec![vec![0, 0, 0, 0, 0, 0, 0, 0]];
        let bytes = d.encode().unwrap();
        let decoded = WirDocument::decode(&bytes).unwrap();
        // Every section 4–8 is decoded (not just CRC-checked) and preserved.
        assert_eq!(decoded.resources, vec![vec![1, 2, 3]]);
        assert_eq!(
            decoded.spatial,
            vec![vec![0xAA], vec![0xBB, 0xCC, 0xDD, 0xEE]]
        );
        assert_eq!(decoded.systems, vec![vec![9, 8, 7]]);
        assert_eq!(decoded.events, vec![vec![42]]);
        assert_eq!(decoded.timelines, vec![vec![0u8; 8]]);
        // And the whole document round-trips byte-identically.
        assert_eq!(decoded.encode().unwrap(), bytes);
    }

    #[test]
    fn wir_record_section_rejects_malformed_framing() {
        // record_count claims 2 records but the body only carries one.
        let mut body = Writer::new();
        body.u32(2).unwrap();
        body.bytes(&[1, 2, 3]).unwrap();
        assert_eq!(
            decode_record_section(&body.finish()).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn wir_crc32c_matches_known_castagnoli_vector() {
        // CRC-32C of "123456789" is the well-known check value 0xE3069283.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn wir_rejects_bad_magic_and_bad_header_crc() {
        let mut bytes = doc().encode().unwrap();
        bytes[0] = b'X';
        assert_eq!(
            WirDocument::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
        // Flip a byte inside the CRC-protected header region.
        let mut bytes = doc().encode().unwrap();
        bytes[20] ^= 0xff;
        assert_eq!(
            WirDocument::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn wir_rejects_out_of_order_or_duplicate_entities() {
        let mut d = doc();
        d.entities.push(EntityRecord {
            id: EntityId(2),
            generation: 1,
            flags: 0,
        });
        d.entities.push(EntityRecord {
            id: EntityId(2),
            generation: 1,
            flags: 0,
        });
        assert_eq!(d.encode().unwrap_err().status, Status::Invalid);
    }

    #[test]
    fn wir_rejects_missing_schema_reference() {
        let mut d = doc();
        d.schemas.clear();
        assert_eq!(d.encode().unwrap_err().status, Status::SchemaHash);
    }

    #[test]
    fn wir_rejects_component_referencing_unknown_entity() {
        let mut d = doc();
        d.components[0].entity = EntityId(999);
        assert_eq!(d.encode().unwrap_err().status, Status::Invalid);
    }

    #[test]
    fn wir_rejects_unknown_section_kind() {
        // Tamper the first directory entry's kind (u16 LE at byte 88) to an
        // unknown value; directory is outside the header-CRC region so the
        // header CRC stays valid and the unknown kind is what must fail.
        let mut bytes = doc().encode().unwrap();
        bytes[88] = 10;
        bytes[89] = 0;
        assert_eq!(
            WirDocument::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
    }

    #[test]
    fn wir_rejects_duplicate_known_section_kind() {
        // Point the first directory entry at ENTITIES (kind=2), duplicating the
        // second entry; a known kind must occur at most once.
        let mut bytes = doc().encode().unwrap();
        bytes[88] = KIND_ENTITIES as u8;
        bytes[89] = 0;
        assert_eq!(
            WirDocument::decode(&bytes).unwrap_err().status,
            Status::Invalid
        );
    }
}
