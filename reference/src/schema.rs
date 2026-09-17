//! RFC-0019 bounded schema registry and canonical schema bytes.

use crate::sha256::digest;
use crate::wire::{Reader, Writer};
use pwe_api::{ComponentTypeId, Error, Hash256, Result, Status};
use std::collections::BTreeMap;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ComponentIdentity {
    pub namespace: String,
    pub stable_name: String,
    pub major_version: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FieldType {
    Bool = 1,
    I32 = 4,
    U32 = 8,
    U64 = 9,
    F32 = 10,
    F64 = 11,
    Bytes = 12,
    String = 13,
    EntityRef = 14,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub id: u32,
    pub name: String,
    pub ty: FieldType,
    pub flags: u16,
}
impl FieldType {
    pub fn tag(self) -> u8 {
        self as u8
    }
    pub fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            1 => Ok(Self::Bool),
            4 => Ok(Self::I32),
            8 => Ok(Self::U32),
            9 => Ok(Self::U64),
            10 => Ok(Self::F32),
            11 => Ok(Self::F64),
            12 => Ok(Self::Bytes),
            13 => Ok(Self::String),
            14 => Ok(Self::EntityRef),
            2 | 3 | 5 | 6 | 7 | 15 | 16 | 17 | 18 | 19 => Err(error(Status::SchemaUnsupported, 1)),
            _ => Err(error(Status::Invalid, 4)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schema {
    pub identity: ComponentIdentity,
    pub fields: Vec<Field>,
}
#[derive(Clone, Debug)]
pub struct RegisteredSchema {
    pub type_id: ComponentTypeId,
    pub hash: Hash256,
    pub schema: Schema,
}

impl ComponentIdentity {
    fn valid_name(value: &str) -> bool {
        let bytes = value.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 128
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(*b, b'.' | b'_' | b'-')
            })
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        if self.major_version == 0
            || !Self::valid_name(&self.namespace)
            || !Self::valid_name(&self.stable_name)
        {
            return Err(error(Status::Invalid, 1));
        }
        let mut out = Writer::new();
        out.u16(self.namespace.len() as u16)?;
        out.bytes_raw(self.namespace.as_bytes())?;
        out.u16(self.stable_name.len() as u16)?;
        out.bytes_raw(self.stable_name.as_bytes())?;
        out.u32(self.major_version)?;
        Ok(out.finish())
    }
    pub fn decode(input: &mut Reader<'_>) -> Result<Self> {
        let namespace_len = input.u16()? as usize;
        let namespace = std::str::from_utf8(input.fixed(namespace_len)?)
            .map_err(|_| error(Status::Invalid, 2))?
            .to_string();
        let name_len = input.u16()? as usize;
        let stable_name = std::str::from_utf8(input.fixed(name_len)?)
            .map_err(|_| error(Status::Invalid, 2))?
            .to_string();
        let major_version = input.u32()?;
        if major_version == 0 || !Self::valid_name(&namespace) || !Self::valid_name(&stable_name) {
            return Err(error(Status::Invalid, 2));
        }
        Ok(Self {
            namespace,
            stable_name,
            major_version,
        })
    }
    pub fn type_id(&self) -> Result<ComponentTypeId> {
        let mut input = b"pwe.component/v2\0".to_vec();
        input.extend(self.canonical_bytes()?);
        let hash = digest(&input);
        Ok(ComponentTypeId(hash.0[..16].try_into().unwrap()))
    }
}
impl Schema {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut fields = self.fields.clone();
        fields.sort_by_key(|field| field.id);
        if fields.len() != self.fields.len()
            || fields
                .windows(2)
                .any(|pair| pair[0].id == 0 || pair[0].id == pair[1].id)
            || fields.last().is_some_and(|field| field.id == 0)
        {
            return Err(error(Status::Invalid, 2));
        }
        let mut out = Writer::new();
        out.u16(2)?;
        let identity = self.identity.canonical_bytes()?;
        out.bytes_raw(&identity)?;
        out.u32(fields.len() as u32)?;
        for field in fields {
            if field.name.is_empty()
                || !field.name.is_ascii()
                || field.name.as_bytes().contains(&0)
                || field.flags & !0x000f != 0
            {
                return Err(error(Status::Invalid, 3));
            }
            out.u32(field.id)?;
            out.u16(field.name.len() as u16)?;
            out.bytes_raw(field.name.as_bytes())?;
            out.u8(field.ty as u8)?;
            out.u16(field.flags)?;
            out.u8(0)?;
        }
        Ok(out.finish())
    }
    /// RFC-0031 fixed size of the schema's canonical ABI value: the sum of its
    /// fixed-size field bytes. Returns `None` if any field is variable-length
    /// (`Bytes`/`String`), for which no single fixed size exists.
    pub fn fixed_size(&self) -> Option<u32> {
        let mut total: u64 = 0;
        for field in &self.fields {
            let size = match field.ty {
                FieldType::Bool => 1u64,
                FieldType::I32 | FieldType::U32 | FieldType::F32 => 4,
                FieldType::U64 | FieldType::F64 => 8,
                FieldType::EntityRef => 16,
                FieldType::Bytes | FieldType::String => return None,
            };
            total += size;
        }
        u32::try_from(total).ok()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut input = Reader::new(bytes)?;
        if input.u16()? != 2 {
            return Err(error(Status::Invalid, 5));
        }
        let identity = ComponentIdentity::decode(&mut input)?;
        let field_count = input.u32()? as usize;
        if field_count > 4096 {
            return Err(error(Status::Limit, 1));
        }
        let mut fields = Vec::with_capacity(field_count);
        let mut previous_id = 0u32;
        for _ in 0..field_count {
            let id = input.u32()?;
            let name_len = input.u16()? as usize;
            let name = std::str::from_utf8(input.fixed(name_len)?)
                .map_err(|_| error(Status::Invalid, 6))?;
            let ty = FieldType::from_tag(input.fixed(1)?[0])?;
            let flags = input.u16()?;
            let has_default = input.fixed(1)?[0];
            if id == 0 || id <= previous_id {
                return Err(error(Status::Invalid, 7));
            }
            if name.is_empty() || !name.is_ascii() || name.as_bytes().contains(&0) {
                return Err(error(Status::Invalid, 8));
            }
            if flags & !0x000f != 0 {
                return Err(error(Status::Invalid, 9));
            }
            if has_default != 0 {
                return Err(error(Status::Invalid, 10));
            }
            previous_id = id;
            fields.push(Field {
                id,
                name: name.to_string(),
                ty,
                flags,
            });
        }
        input.finish()?;
        Ok(Self { identity, fields })
    }
    pub fn hash(&self) -> Result<Hash256> {
        Ok(digest(&self.canonical_bytes()?))
    }
}
#[derive(Default, Debug)]
pub struct SchemaRegistry {
    schemas: BTreeMap<ComponentTypeId, RegisteredSchema>,
}
impl SchemaRegistry {
    pub fn register(&mut self, schema: Schema) -> Result<&RegisteredSchema> {
        let type_id = schema.identity.type_id()?;
        let hash = schema.hash()?;
        if let Some(existing_hash) = self.schemas.get(&type_id).map(|existing| existing.hash) {
            if existing_hash != hash {
                return Err(error(Status::HashCollision, 1));
            }
            return Ok(self.schemas.get(&type_id).unwrap());
        }
        self.schemas.insert(
            type_id,
            RegisteredSchema {
                type_id,
                hash,
                schema,
            },
        );
        Ok(self.schemas.get(&type_id).unwrap())
    }
    pub fn get(&self, type_id: ComponentTypeId) -> Option<&RegisteredSchema> {
        self.schemas.get(&type_id)
    }
    /// RFC-0031: validates a component descriptor against the registered schema
    /// it claims (by `type_id`). Enforces `value_size == schema_size` for
    /// fixed-size schemas and verifies the descriptor's `schema_hash` matches
    /// the schema's canonical hash. Returns `Err(SchemaHash)` if the type is
    /// unregistered or the schema hash mismatches, and `Err(AbiMismatch)` on a
    /// size violation.
    pub fn validate_descriptor(&self, descriptor: &pwe_api::ComponentDescriptor) -> Result<()> {
        let registered = self
            .get(descriptor.type_id)
            .ok_or(error(Status::SchemaHash, 3))?;
        descriptor
            .validate_against(registered.schema.fixed_size(), registered.hash)
            .map(|_| ())
    }
    pub fn schema_set_hash(&self) -> Hash256 {
        let mut input = b"pwe.schema-set/v2\0".to_vec();
        let mut hashes: Vec<_> = self.schemas.values().map(|schema| schema.hash).collect();
        hashes.sort();
        for hash in hashes {
            input.extend(hash.0);
        }
        digest(&input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn schema() -> Schema {
        Schema {
            identity: ComponentIdentity {
                namespace: "pwe.physics".into(),
                stable_name: "body".into(),
                major_version: 1,
            },
            fields: vec![
                Field {
                    id: 2,
                    name: "mass".into(),
                    ty: FieldType::F32,
                    flags: 1,
                },
                Field {
                    id: 1,
                    name: "id".into(),
                    ty: FieldType::U64,
                    flags: 1,
                },
            ],
        }
    }
    #[test]
    fn schema_canonical_order_and_registry_are_stable() {
        let mut first = SchemaRegistry::default();
        let registered = first.register(schema()).unwrap().clone();
        let mut second = SchemaRegistry::default();
        second.register(schema()).unwrap();
        assert_eq!(registered.hash, schema().hash().unwrap());
        assert_eq!(first.schema_set_hash(), second.schema_set_hash());
    }
    #[test]
    fn invalid_schema_is_rejected() {
        let mut invalid = schema();
        invalid.fields.push(Field {
            id: 1,
            name: "again".into(),
            ty: FieldType::U32,
            flags: 0,
        });
        assert_eq!(invalid.hash().unwrap_err().status, Status::Invalid);
    }
    #[test]
    fn fixed_size_sum_of_fields_and_variable_size_is_none() {
        // id(U64=8) + mass(F32=4) = 12 bytes.
        assert_eq!(schema().fixed_size(), Some(12));
        let mut var = schema();
        var.fields.push(Field {
            id: 3,
            name: "label".into(),
            ty: FieldType::String,
            flags: 0,
        });
        assert_eq!(var.fixed_size(), None);
    }
    #[test]
    fn validate_descriptor_enforces_size_and_schema_hash() {
        let mut registry = SchemaRegistry::default();
        registry.register(schema()).unwrap();
        let registered = registry.get(schema().identity.type_id().unwrap()).unwrap();
        let good = pwe_api::ComponentDescriptor {
            type_id: registered.type_id,
            schema_hash: registered.hash,
            abi_major: pwe_api::COMPONENT_ABI_MAJOR,
            flags: 0,
            value_size: 12, // matches the fixed schema size
            value_align: 4,
        };
        assert!(registry.validate_descriptor(&good).is_ok());
        // value_size != schema_size is rejected.
        let mut bad_size = good;
        bad_size.value_size = 13;
        assert_eq!(
            registry.validate_descriptor(&bad_size).unwrap_err().status,
            Status::AbiMismatch
        );
        // Wrong schema hash is rejected.
        let mut bad_hash = good;
        bad_hash.schema_hash = Hash256([0xAB; 32]);
        assert_eq!(
            registry.validate_descriptor(&bad_hash).unwrap_err().status,
            Status::AbiMismatch
        );
        // Unregistered type is rejected.
        let mut unknown = good;
        unknown.type_id = ComponentTypeId([9; 16]);
        assert_eq!(
            registry.validate_descriptor(&unknown).unwrap_err().status,
            Status::SchemaHash
        );
    }
    #[test]
    fn variable_size_schema_descriptor_skips_size_check_but_checks_hash() {
        let mut registry = SchemaRegistry::default();
        let mut var = schema();
        var.fields.push(Field {
            id: 3,
            name: "label".into(),
            ty: FieldType::String,
            flags: 0,
        });
        registry.register(var.clone()).unwrap();
        let registered = registry.get(var.identity.type_id().unwrap()).unwrap();
        // fixed_size() is None, so any value_size passes the size gate; the
        // hash is still enforced.
        let desc = pwe_api::ComponentDescriptor {
            type_id: registered.type_id,
            schema_hash: registered.hash,
            abi_major: pwe_api::COMPONENT_ABI_MAJOR,
            flags: 0,
            value_size: 999,
            value_align: 8,
        };
        assert!(registry.validate_descriptor(&desc).is_ok());
    }
}
