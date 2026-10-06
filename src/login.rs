//! Open at login, through macOS's SMAppService (macOS 13+). Only works from
//! the app bundle; the login item is the app itself.
#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use cocoa::base::{id, nil};
use objc::{class, msg_send, sel, sel_impl};

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

fn service() -> id {
    unsafe { msg_send![class!(SMAppService), mainAppService] }
}

/// Is Heyra set to open at login?
pub fn enabled() -> bool {
    // SMAppServiceStatus: 0 not registered, 1 enabled, 2 needs approval, 3 not found.
    let status: isize = unsafe { msg_send![service(), status] };
    status == 1
}

pub fn set(on: bool) {
    let mut error: id = nil;
    let ok: bool = unsafe {
        if on {
            msg_send![service(), registerAndReturnError: &mut error]
        } else {
            msg_send![service(), unregisterAndReturnError: &mut error]
        }
    };
    crate::store::log(&format!("open at login {}: {}", if on { "on" } else { "off" }, if ok { "ok" } else { "failed" }));
}
