//! Keeps the display awake while a video plays, on Windows and macOS. On
//! Linux GTK's inhibit (portal or session manager) does it; GTK has no such
//! backend on the other two, so the system is asked directly.

use std::sync::Mutex;

#[cfg(windows)]
mod system {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    /// Lasts while this thread lives or until `release`; always the main
    /// thread here.
    pub fn hold() -> Option<()> {
        // SAFETY: plain flags, no pointers.
        let previous = unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
        };
        (previous != 0).then_some(())
    }

    pub fn release(_: ()) {
        // SAFETY: as above.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

#[cfg(target_os = "macos")]
mod system {
    use std::ffi::{c_char, c_void};

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ASSERTION_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(object: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// An IOKit power assertion: no display sleep (and so no idle system
    /// sleep) until released.
    pub fn hold() -> Option<u32> {
        // SAFETY: NUL-terminated literals; each string is released after use.
        unsafe {
            let kind = CFStringCreateWithCString(
                std::ptr::null(),
                c"PreventUserIdleDisplaySleep".as_ptr(),
                UTF8,
            );
            let name =
                CFStringCreateWithCString(std::ptr::null(), c"Playing a video".as_ptr(), UTF8);
            if kind.is_null() || name.is_null() {
                for string in [kind, name] {
                    if !string.is_null() {
                        CFRelease(string);
                    }
                }
                return None;
            }
            let mut id = 0;
            let status = IOPMAssertionCreateWithName(kind, ASSERTION_ON, name, &mut id);
            CFRelease(kind);
            CFRelease(name);
            (status == 0).then_some(id)
        }
    }

    pub fn release(id: u32) {
        // SAFETY: an assertion created by `hold`, released once.
        unsafe { IOPMAssertionRelease(id) };
    }
}

#[cfg(windows)]
type Held = ();
#[cfg(target_os = "macos")]
type Held = u32;

static HELD: Mutex<Option<Held>> = Mutex::new(None);

/// Keeps the display awake (`true`) or lets it sleep again (`false`).
pub fn set(on: bool) {
    let Ok(mut held) = HELD.lock() else { return };
    if on && held.is_none() {
        *held = system::hold();
        if held.is_none() {
            tracing::warn!("couldn't keep the display awake");
        }
    } else if !on && let Some(state) = held.take() {
        system::release(state);
    }
}
