//! Keeps the display awake while a video plays where GTK's inhibit (portal
//! or session manager) doesn't reach: Windows, macOS and Steam's Game Mode.

use std::sync::Mutex;

#[cfg(target_os = "linux")]
mod system {
    use gtk::gio;
    use gtk::glib::{self, prelude::*};

    const NAME: &str = "org.freedesktop.ScreenSaver";
    const PATH: &str = "/org/freedesktop/ScreenSaver";

    /// In Game Mode Steam owns the session's ScreenSaver name and runs its own
    /// dim/sleep timer; GTK never calls it. The inhibit lasts as long as this
    /// connection, so it's kept with the cookie.
    pub fn hold() -> Option<(gio::DBusConnection, u32)> {
        if !crate::gamescope::detected() {
            return None;
        }
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
        let reply = bus
            .call_sync(
                Some(NAME),
                PATH,
                NAME,
                "Inhibit",
                Some(&(crate::APP_ID, "Playing a video").to_variant()),
                Some(glib::VariantTy::new("(u)").ok()?),
                gio::DBusCallFlags::NONE,
                2000,
                gio::Cancellable::NONE,
            )
            .inspect_err(|e| tracing::warn!("ScreenSaver.Inhibit failed: {e}"))
            .ok()?;
        let (cookie,) = reply.get::<(u32,)>()?;
        Some((bus, cookie))
    }

    pub fn release((bus, cookie): (gio::DBusConnection, u32)) {
        bus.call(
            Some(NAME),
            PATH,
            NAME,
            "UnInhibit",
            Some(&(cookie,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
            |result| {
                if let Err(e) = result {
                    tracing::warn!("ScreenSaver.UnInhibit failed: {e}");
                }
            },
        );
    }
}

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
#[cfg(target_os = "linux")]
type Held = (gtk::gio::DBusConnection, u32);

static HELD: Mutex<Option<Held>> = Mutex::new(None);

/// Keeps the display awake (`true`) or lets it sleep again (`false`).
pub fn set(on: bool) {
    let Ok(mut held) = HELD.lock() else { return };
    if on && held.is_none() {
        *held = system::hold();
        // Outside Game Mode, Linux has nothing to do here.
        if held.is_none() && (cfg!(not(target_os = "linux")) || crate::gamescope::detected()) {
            tracing::warn!("couldn't keep the display awake");
        }
    } else if !on && let Some(state) = held.take() {
        system::release(state);
    }
}
