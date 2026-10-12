//! macOS key suppression: swallow hotkey presses made during a recording.
//!
//! keytap is observe-only, so a plain key bound as a hotkey (e.g. `F` as a
//! prompt's stop key) both fires the hotkey and types into the focused app.
//! This module runs a second, *active* `CGEventTap` that drops such presses
//! while a recording is in progress.
//!
//! - The tap sits at the session level, downstream of keytap's HID-level tap,
//!   so keytap (and thus the matcher) still observes every swallowed key.
//! - Only `KeyDown`/`KeyUp` are tapped. Modifiers arrive as `FlagsChanged` and
//!   are never swallowed, so the system modifier state stays consistent.
//! - An active tap needs the Accessibility permission. Without it the tap
//!   cannot be created and keys simply pass through as before.

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::{mpsc, Mutex};

use keytap::Key;
use objc2_core_foundation::{kCFRunLoopCommonModes, CFMachPort, CFRetained, CFRunLoop};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType,
};

use super::HotkeyConfig;

/// Whether the suppression tap is running. Guards against a second tap when
/// startup and the post-permission reinit race.
static STARTED: Mutex<bool> = Mutex::new(false);

/// State owned by the tap thread and reached from the C callback through
/// `user_info`. Never shared across threads, hence the plain cells.
struct TapContext {
    config: HotkeyConfig,
    /// Keys whose keydown was swallowed; their repeats and keyup must be
    /// swallowed too so the focused app never sees half a keystroke.
    swallowed: RefCell<HashSet<Key>>,
    /// The tap's own port, needed to re-enable it after the system disables it.
    port: OnceCell<CFRetained<CFMachPort>>,
}

/// Start the suppression tap if it is not running yet. Safe to call
/// repeatedly; a failed attempt (missing Accessibility permission) is retried
/// on the next call.
pub(super) fn ensure_started(config: &HotkeyConfig) {
    let mut started = STARTED.lock().unwrap();
    if *started {
        return;
    }

    let (ready_tx, ready_rx) = mpsc::channel::<bool>();
    let config = config.clone();
    let spawned = std::thread::Builder::new()
        .name("voicepaste-key-suppress".into())
        .spawn(move || run_tap_thread(config, ready_tx));
    if let Err(e) = spawned {
        log_hotkey!(warn, "Failed to spawn key suppression thread: {}", e);
        return;
    }

    if ready_rx.recv().unwrap_or(false) {
        *started = true;
        log_hotkey!(info, "Key suppression tap started");
    } else {
        log_hotkey!(
            warn,
            "Key suppression tap unavailable (Accessibility permission?) — hotkey keys will also reach the focused app"
        );
    }
}

/// Create the tap and run its run loop. Runs for the app lifetime once the
/// tap is created, like the hotkey listener itself.
fn run_tap_thread(config: HotkeyConfig, ready_tx: mpsc::Sender<bool>) {
    let ctx_ptr = Box::into_raw(Box::new(TapContext {
        config,
        swallowed: RefCell::new(HashSet::new()),
        port: OnceCell::new(),
    }));

    let mask: u64 = (1u64 << CGEventType::KeyDown.0) | (1u64 << CGEventType::KeyUp.0);
    // SAFETY: `ctx_ptr` is a valid heap allocation that outlives the tap (it
    // is only freed below, on paths where the tap never ran).
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            mask,
            Some(tap_callback),
            ctx_ptr as *mut c_void,
        )
    };
    let source = tap
        .as_ref()
        .and_then(|tap| CFMachPort::new_run_loop_source(None, Some(tap), 0));
    let run_loop = CFRunLoop::current();

    let (Some(tap), Some(source), Some(run_loop)) = (tap, source, run_loop) else {
        // SAFETY: the tap was never added to a run loop, so the callback
        // cannot run and nothing else holds the pointer.
        unsafe { drop(Box::from_raw(ctx_ptr)) };
        let _ = ready_tx.send(false);
        return;
    };

    // SAFETY: the callback only runs from this thread's run loop, which has
    // not started yet, so this is the only live reference.
    let _ = unsafe { &*ctx_ptr }.port.set(tap.clone());

    // SAFETY: `kCFRunLoopCommonModes` is an immutable CoreFoundation constant.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    CGEvent::tap_enable(&tap, true);
    let _ = ready_tx.send(true);

    CFRunLoop::run();
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    cg_event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user_info` is the leaked `TapContext` installed in
    // `run_tap_thread`; it lives as long as the tap and is only touched here,
    // on the tap thread.
    let ctx: &TapContext = unsafe { &*(user_info as *const TapContext) };
    let pass = cg_event.as_ptr();

    // The system disables a tap whose callback was too slow (or on secure
    // input). Re-enable it and forget in-flight keys whose keyup we may miss.
    if event_type == CGEventType::TapDisabledByTimeout
        || event_type == CGEventType::TapDisabledByUserInput
    {
        ctx.swallowed.borrow_mut().clear();
        if let Some(port) = ctx.port.get() {
            CGEvent::tap_enable(port, true);
        }
        return pass;
    }

    // SAFETY: the system hands the callback a valid event for its duration.
    let event: &CGEvent = unsafe { cg_event.as_ref() };
    let keycode = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode);
    let Some(key) = key_from_keycode(keycode) else {
        return pass;
    };

    let mut swallowed = ctx.swallowed.borrow_mut();
    let swallow = match event_type {
        CGEventType::KeyDown => {
            let is_repeat =
                CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventAutorepeat)
                    != 0;
            if is_repeat {
                swallowed.contains(&key)
            } else {
                let suppress = ctx
                    .config
                    .read()
                    .map(|cfg| cfg.matcher.should_suppress(key, &cfg.bindings))
                    .unwrap_or(false);
                if suppress {
                    swallowed.insert(key);
                } else {
                    swallowed.remove(&key);
                }
                suppress
            }
        }
        CGEventType::KeyUp => swallowed.remove(&key),
        _ => false,
    };

    if swallow {
        ptr::null_mut()
    } else {
        pass
    }
}

/// Map a macOS virtual keycode (`kVK_*`) to its keytap [`Key`], mirroring
/// keytap's private table so both taps agree on key identity. Modifiers are
/// omitted: they never arrive as `KeyDown`/`KeyUp`.
fn key_from_keycode(code: i64) -> Option<Key> {
    Some(match code {
        0 => Key::A,
        1 => Key::S,
        2 => Key::D,
        3 => Key::F,
        4 => Key::H,
        5 => Key::G,
        6 => Key::Z,
        7 => Key::X,
        8 => Key::C,
        9 => Key::V,
        10 => Key::IntlBackslash,
        11 => Key::B,
        12 => Key::Q,
        13 => Key::W,
        14 => Key::E,
        15 => Key::R,
        16 => Key::Y,
        17 => Key::T,
        18 => Key::Digit1,
        19 => Key::Digit2,
        20 => Key::Digit3,
        21 => Key::Digit4,
        22 => Key::Digit6,
        23 => Key::Digit5,
        24 => Key::Equal,
        25 => Key::Digit9,
        26 => Key::Digit7,
        27 => Key::Minus,
        28 => Key::Digit8,
        29 => Key::Digit0,
        30 => Key::BracketRight,
        31 => Key::O,
        32 => Key::U,
        33 => Key::BracketLeft,
        34 => Key::I,
        35 => Key::P,
        36 => Key::Enter,
        37 => Key::L,
        38 => Key::J,
        39 => Key::Quote,
        40 => Key::K,
        41 => Key::Semicolon,
        42 => Key::Backslash,
        43 => Key::Comma,
        44 => Key::Slash,
        45 => Key::N,
        46 => Key::M,
        47 => Key::Period,
        48 => Key::Tab,
        49 => Key::Space,
        50 => Key::Backtick,
        51 => Key::Backspace,
        53 => Key::Escape,
        64 => Key::F17,
        65 => Key::NumpadDecimal,
        67 => Key::NumpadMultiply,
        69 => Key::NumpadAdd,
        71 => Key::NumLock,
        75 => Key::NumpadDivide,
        76 => Key::NumpadEnter,
        78 => Key::NumpadSubtract,
        79 => Key::F18,
        80 => Key::F19,
        82 => Key::Numpad0,
        83 => Key::Numpad1,
        84 => Key::Numpad2,
        85 => Key::Numpad3,
        86 => Key::Numpad4,
        87 => Key::Numpad5,
        88 => Key::Numpad6,
        89 => Key::Numpad7,
        90 => Key::F20,
        91 => Key::Numpad8,
        92 => Key::Numpad9,
        96 => Key::F5,
        97 => Key::F6,
        98 => Key::F7,
        99 => Key::F3,
        100 => Key::F8,
        101 => Key::F9,
        103 => Key::F11,
        105 => Key::F13,
        106 => Key::F16,
        107 => Key::F14,
        109 => Key::F10,
        111 => Key::F12,
        113 => Key::F15,
        115 => Key::Home,
        116 => Key::PageUp,
        117 => Key::Delete,
        118 => Key::F4,
        119 => Key::End,
        120 => Key::F2,
        121 => Key::PageDown,
        122 => Key::F1,
        123 => Key::ArrowLeft,
        124 => Key::ArrowRight,
        125 => Key::ArrowDown,
        126 => Key::ArrowUp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_plain_keys() {
        assert_eq!(key_from_keycode(3), Some(Key::F));
        assert_eq!(key_from_keycode(49), Some(Key::Space));
        assert_eq!(key_from_keycode(105), Some(Key::F13));
        assert_eq!(key_from_keycode(126), Some(Key::ArrowUp));
    }

    #[test]
    fn modifiers_and_unknown_codes_are_not_mapped() {
        // kVK_Command, kVK_Shift, kVK_CapsLock, kVK_Option, kVK_Control,
        // right-hand variants, kVK_Function.
        for code in [54, 55, 56, 57, 58, 59, 60, 61, 62, 63] {
            assert_eq!(key_from_keycode(code), None, "keycode {code}");
        }
        assert_eq!(key_from_keycode(9999), None);
    }
}
