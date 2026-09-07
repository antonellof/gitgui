//! macOS only: the dock icon for a bare binary. A process outside an .app
//! bundle gets a generic icon; `NSApplication setApplicationIconImage:`
//! replaces it at runtime. Raw Objective-C messaging over the libobjc that
//! winit already links, so no extra crate.

use std::ffi::{c_char, c_void, CString};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

unsafe fn class(name: &str) -> Id {
    let c = CString::new(name).expect("class name");
    objc_getClass(c.as_ptr())
}

unsafe fn sel(name: &str) -> Sel {
    let c = CString::new(name).expect("selector");
    sel_registerName(c.as_ptr())
}

unsafe fn send0(obj: Id, s: &str) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, sel(s))
}

unsafe fn send1(obj: Id, s: &str, a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, sel(s), a)
}

unsafe fn send2(obj: Id, s: &str, a: *const c_void, b: usize) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, *const c_void, usize) -> Id =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, sel(s), a, b)
}

/// Show `png` as this process's dock icon. Silently does nothing if any
/// step fails; the icon is cosmetic.
pub fn set_dock_icon(png: &'static [u8]) {
    // SAFETY: plain AppKit calls on the main thread with owned objects
    // (alloc + init), which AppKit retains; the byte slice is 'static.
    unsafe {
        let data = send0(class("NSData"), "alloc");
        if data.is_null() {
            return;
        }
        let data = send2(data, "initWithBytes:length:", png.as_ptr() as *const c_void, png.len());
        let image = send0(class("NSImage"), "alloc");
        if data.is_null() || image.is_null() {
            return;
        }
        let image = send1(image, "initWithData:", data);
        if image.is_null() {
            return;
        }
        let app = send0(class("NSApplication"), "sharedApplication");
        if !app.is_null() {
            send1(app, "setApplicationIconImage:", image);
        }
    }
}
