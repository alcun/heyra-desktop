//! The Mac's microphones and what kind each is, straight from CoreAudio: so the
//! list can put real mics first, and a phone or virtual device that appears
//! doesn't take over.

use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    BuiltIn,
    /// USB, Thunderbolt and other cables: a plugged-in mic or adapter.
    Wired,
    Bluetooth,
    /// iPhone (Continuity), virtual and aggregate devices.
    Other,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Mic {
    pub name: String,
    pub kind: Kind,
}

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: *mut u32) -> i32;
    fn AudioObjectGetPropertyData(object: u32, address: *const Address, qualifier_size: u32, qualifier: *const std::ffi::c_void, size: *mut u32, data: *mut std::ffi::c_void) -> i32;
}

const fn code(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM: u32 = 1;
const GLOBAL: u32 = code(b"glob");
const INPUT: u32 = code(b"inpt");

fn size(object: u32, selector: u32, scope: u32) -> u32 {
    let address = Address { selector, scope, element: 0 };
    let mut size = 0;
    let status = unsafe { AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) };
    if status == 0 { size } else { 0 }
}

fn get<T: Copy + Default>(object: u32, selector: u32) -> Option<T> {
    let address = Address { selector, scope: GLOBAL, element: 0 };
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(object, &address, 0, std::ptr::null(), &mut size, &mut value as *mut T as *mut _)
    };
    (status == 0).then_some(value)
}

fn name(device: u32) -> Option<String> {
    let raw: usize = get(device, code(b"lnam"))?;
    (raw != 0).then(|| unsafe { CFString::wrap_under_create_rule(raw as CFStringRef) }.to_string())
}

fn kind(device: u32) -> Kind {
    match &get::<u32>(device, code(b"tran")).unwrap_or(0).to_be_bytes() {
        b"bltn" => Kind::BuiltIn,
        b"usb " | b"thun" | b"1394" | b"pci " | b"hdmi" | b"dprt" | b"avb " => Kind::Wired,
        b"blue" | b"blea" => Kind::Bluetooth,
        _ => Kind::Other,
    }
}

/// Every device with an input, real mics first (built-in, then wired, then Bluetooth).
pub fn list() -> Vec<Mic> {
    let bytes = size(SYSTEM, code(b"dev#"), GLOBAL);
    let mut ids = vec![0u32; bytes as usize / 4];
    let address = Address { selector: code(b"dev#"), scope: GLOBAL, element: 0 };
    let mut got = bytes;
    let status = unsafe {
        AudioObjectGetPropertyData(SYSTEM, &address, 0, std::ptr::null(), &mut got, ids.as_mut_ptr() as *mut _)
    };
    if status != 0 {
        return Vec::new();
    }
    let mut mics: Vec<Mic> = ids
        .into_iter()
        .filter(|&id| size(id, code(b"stm#"), INPUT) > 0)
        .filter_map(|id| Some(Mic { name: name(id)?, kind: kind(id) }))
        .collect();
    mics.sort_by_key(|m| m.kind as u8);
    mics
}

/// The input macOS's Sound settings point at.
pub fn default_name() -> Option<String> {
    name(get(SYSTEM, code(b"dIn "))?)
}
