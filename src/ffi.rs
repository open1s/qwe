//! RFC-0026 C11 ABI. Rust traits in the parent module are not this ABI.

use core::ffi::c_void;

pub const ABI_MAJOR: u16 = 2;
pub const ABI_MINOR: u16 = 0;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PweStatus(pub u32);
pub const OK: PweStatus = PweStatus(0);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PweBuffer {
    pub ptr: *const u8,
    pub len: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PweMutableBuffer {
    pub ptr: *mut u8,
    pub len: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PweError {
    pub status: PweStatus,
    pub detail: u32,
    pub byte_offset: u64,
    pub message: PweBuffer,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PweHandle {
    pub kind: u32,
    pub index: u32,
    pub generation: u32,
    pub reserved: u32,
}

pub type WorldGet =
    unsafe extern "C" fn(*mut c_void, PweBuffer, *mut PweHandle, *mut PweError) -> PweStatus;
pub type ComponentView = unsafe extern "C" fn(
    *mut c_void,
    PweHandle,
    PweBuffer,
    *mut PweBuffer,
    *mut PweError,
) -> PweStatus;
pub type ResourceGet =
    unsafe extern "C" fn(*mut c_void, PweBuffer, *mut PweBuffer, *mut PweError) -> PweStatus;
pub type TransactionBegin = unsafe extern "C" fn(
    *mut c_void,
    PweBuffer,
    PweBuffer,
    *mut PweHandle,
    *mut PweError,
) -> PweStatus;
pub type TransactionSubmit = unsafe extern "C" fn(
    *mut c_void,
    PweHandle,
    PweBuffer,
    *mut PweBuffer,
    *mut PweError,
) -> PweStatus;
pub type HandleRelease = unsafe extern "C" fn(*mut c_void, PweHandle, *mut PweError) -> PweStatus;
pub type RuntimeFree = unsafe extern "C" fn(*mut c_void, PweMutableBuffer);

/// RFC-0016 plugin/backend ABI. Extensions load through a stable C ABI and PWE
/// schema; they receive capability-bound views/handles, never unrestricted
/// world pointers.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PweComponentDescriptorC {
    pub type_id: [u8; 16],
    pub schema_hash: [u8; 32],
    pub abi_major: u32,
    pub flags: u32,
    pub value_size: u32,
    pub value_align: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PweSystemDescriptorC {
    pub id: [u8; 16],
    pub effects: u32,
    pub access_mask: u32,
    pub priority: u32,
    pub reserved: u32,
}

pub type PluginRegisterComponent =
    unsafe extern "C" fn(*mut c_void, *const PweComponentDescriptorC, *mut PweError) -> PweStatus;
pub type PluginRegisterSystem =
    unsafe extern "C" fn(*mut c_void, *const PweSystemDescriptorC, *mut PweError) -> PweStatus;

/// Versioned, size-extensible plugin function table (RFC-0016 §7).
#[repr(C)]
pub struct PwePluginApi {
    pub abi_major: u16,
    pub abi_minor: u16,
    pub size: u32,
    pub user_data: *mut c_void,
    pub register_component: Option<PluginRegisterComponent>,
    pub register_system: Option<PluginRegisterSystem>,
}

impl PwePluginApi {
    pub fn is_compatible(&self) -> bool {
        self.abi_major == ABI_MAJOR && (self.size as usize) >= core::mem::size_of::<PwePluginApi>()
    }
}

/// `size` is the size supplied by the provider, allowing append-only ABI growth.
#[repr(C)]
pub struct RuntimeApi {
    pub abi_major: u16,
    pub abi_minor: u16,
    pub size: u32,
    pub user_data: *mut c_void,
    pub world_get: Option<WorldGet>,
    pub component_view: Option<ComponentView>,
    pub resource_get: Option<ResourceGet>,
    pub transaction_begin: Option<TransactionBegin>,
    pub transaction_submit: Option<TransactionSubmit>,
    pub handle_release: Option<HandleRelease>,
    pub runtime_free: Option<RuntimeFree>,
}

impl RuntimeApi {
    pub fn is_compatible(&self) -> bool {
        self.abi_major == ABI_MAJOR && (self.size as usize) >= core::mem::size_of::<RuntimeApi>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handle_is_fixed_width() {
        assert_eq!(core::mem::size_of::<PweHandle>(), 16);
        assert_eq!(core::mem::size_of::<PweBuffer>(), 16);
    }
    #[test]
    fn error_layout_matches_c_header() {
        // status:u32 detail:u32 byte_offset:u64 message:{ptr,len}
        assert_eq!(core::mem::size_of::<PweError>(), 32);
        assert_eq!(core::mem::offset_of!(PweError, byte_offset), 8);
        assert_eq!(core::mem::offset_of!(PweError, message), 16);
        assert_eq!(core::mem::size_of::<PweStatus>(), 4);
    }
    #[test]
    fn runtime_api_layout_matches_c_header() {
        // u16 + u16 + u32 + 7 pointer-sized slots (user_data + 7 fn slots + ...)
        assert_eq!(core::mem::offset_of!(RuntimeApi, user_data), 8);
        assert_eq!(core::mem::offset_of!(RuntimeApi, world_get), 16);
        assert_eq!(core::mem::offset_of!(RuntimeApi, component_view), 24);
        assert_eq!(core::mem::offset_of!(RuntimeApi, resource_get), 32);
        assert_eq!(core::mem::offset_of!(RuntimeApi, transaction_begin), 40);
        assert_eq!(core::mem::offset_of!(RuntimeApi, transaction_submit), 48);
        assert_eq!(core::mem::offset_of!(RuntimeApi, handle_release), 56);
        assert_eq!(core::mem::offset_of!(RuntimeApi, runtime_free), 64);
        assert_eq!(core::mem::size_of::<RuntimeApi>(), 72);
    }
}
