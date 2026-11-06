//! W3C key codes and modifiers in macOS terms.

use keyboard_types::{Code, Modifiers};
use objc2_core_graphics::{CGEventFlags, CGKeyCode};

use crate::Error;

/// The macOS virtual key code (`kVK_*` in `HIToolbox/Events.h`) of the key
/// at `code`.
#[expect(clippy::too_many_lines, reason = "one arm per mapped key")]
pub const fn virtual_key(code: Code) -> Result<CGKeyCode, Error> {
    Ok(match code {
        Code::KeyA => 0x00,
        Code::KeyS => 0x01,
        Code::KeyD => 0x02,
        Code::KeyF => 0x03,
        Code::KeyH => 0x04,
        Code::KeyG => 0x05,
        Code::KeyZ => 0x06,
        Code::KeyX => 0x07,
        Code::KeyC => 0x08,
        Code::KeyV => 0x09,
        Code::IntlBackslash => 0x0A,
        Code::KeyB => 0x0B,
        Code::KeyQ => 0x0C,
        Code::KeyW => 0x0D,
        Code::KeyE => 0x0E,
        Code::KeyR => 0x0F,
        Code::KeyY => 0x10,
        Code::KeyT => 0x11,
        Code::Digit1 => 0x12,
        Code::Digit2 => 0x13,
        Code::Digit3 => 0x14,
        Code::Digit4 => 0x15,
        Code::Digit6 => 0x16,
        Code::Digit5 => 0x17,
        Code::Equal => 0x18,
        Code::Digit9 => 0x19,
        Code::Digit7 => 0x1A,
        Code::Minus => 0x1B,
        Code::Digit8 => 0x1C,
        Code::Digit0 => 0x1D,
        Code::BracketRight => 0x1E,
        Code::KeyO => 0x1F,
        Code::KeyU => 0x20,
        Code::BracketLeft => 0x21,
        Code::KeyI => 0x22,
        Code::KeyP => 0x23,
        Code::Enter => 0x24,
        Code::KeyL => 0x25,
        Code::KeyJ => 0x26,
        Code::Quote => 0x27,
        Code::KeyK => 0x28,
        Code::Semicolon => 0x29,
        Code::Backslash => 0x2A,
        Code::Comma => 0x2B,
        Code::Slash => 0x2C,
        Code::KeyN => 0x2D,
        Code::KeyM => 0x2E,
        Code::Period => 0x2F,
        Code::Tab => 0x30,
        Code::Space => 0x31,
        Code::Backquote => 0x32,
        Code::Backspace => 0x33,
        Code::Escape => 0x35,
        Code::MetaRight => 0x36,
        Code::MetaLeft => 0x37,
        Code::ShiftLeft => 0x38,
        Code::CapsLock => 0x39,
        Code::AltLeft => 0x3A,
        Code::ControlLeft => 0x3B,
        Code::ShiftRight => 0x3C,
        Code::AltRight => 0x3D,
        Code::ControlRight => 0x3E,
        Code::Fn => 0x3F,
        Code::F17 => 0x40,
        Code::NumpadDecimal => 0x41,
        Code::NumpadMultiply => 0x43,
        Code::NumpadAdd => 0x45,
        Code::NumLock => 0x47,
        Code::AudioVolumeUp => 0x48,
        Code::AudioVolumeDown => 0x49,
        Code::AudioVolumeMute => 0x4A,
        Code::NumpadDivide => 0x4B,
        Code::NumpadEnter => 0x4C,
        Code::NumpadSubtract => 0x4E,
        Code::F18 => 0x4F,
        Code::F19 => 0x50,
        Code::NumpadEqual => 0x51,
        Code::Numpad0 => 0x52,
        Code::Numpad1 => 0x53,
        Code::Numpad2 => 0x54,
        Code::Numpad3 => 0x55,
        Code::Numpad4 => 0x56,
        Code::Numpad5 => 0x57,
        Code::Numpad6 => 0x58,
        Code::Numpad7 => 0x59,
        Code::F20 => 0x5A,
        Code::Numpad8 => 0x5B,
        Code::Numpad9 => 0x5C,
        Code::IntlYen => 0x5D,
        Code::IntlRo => 0x5E,
        Code::NumpadComma => 0x5F,
        Code::F5 => 0x60,
        Code::F6 => 0x61,
        Code::F7 => 0x62,
        Code::F3 => 0x63,
        Code::F8 => 0x64,
        Code::F9 => 0x65,
        Code::Lang2 => 0x66,
        Code::F11 => 0x67,
        Code::Lang1 => 0x68,
        Code::F13 => 0x69,
        Code::F16 => 0x6A,
        Code::F14 => 0x6B,
        Code::F10 => 0x6D,
        Code::ContextMenu => 0x6E,
        Code::F12 => 0x6F,
        Code::F15 => 0x71,
        Code::Help => 0x72,
        Code::Home => 0x73,
        Code::PageUp => 0x74,
        Code::Delete => 0x75,
        Code::F4 => 0x76,
        Code::End => 0x77,
        Code::F2 => 0x78,
        Code::PageDown => 0x79,
        Code::F1 => 0x7A,
        Code::ArrowLeft => 0x7B,
        Code::ArrowRight => 0x7C,
        Code::ArrowDown => 0x7D,
        Code::ArrowUp => 0x7E,
        _ => return Err(Error::UnmappableKey(code)),
    })
}

/// The event flags for `modifiers`.
pub fn event_flags(modifiers: Modifiers) -> Result<CGEventFlags, Error> {
    const MAPPED: [(Modifiers, CGEventFlags); 6] = [
        (Modifiers::SHIFT, CGEventFlags::MaskShift),
        (Modifiers::CONTROL, CGEventFlags::MaskControl),
        (Modifiers::ALT, CGEventFlags::MaskAlternate),
        (Modifiers::META, CGEventFlags::MaskCommand),
        (Modifiers::CAPS_LOCK, CGEventFlags::MaskAlphaShift),
        (Modifiers::FN, CGEventFlags::MaskSecondaryFn),
    ];
    let mut flags = CGEventFlags::empty();
    let mut unmapped = modifiers;
    for (modifier, flag) in MAPPED {
        if modifiers.contains(modifier) {
            flags |= flag;
            unmapped.remove(modifier);
        }
    }
    if unmapped.is_empty() {
        Ok(flags)
    } else {
        Err(Error::UnmappableModifiers(unmapped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_their_ansi_virtual_key_codes() {
        assert_eq!(virtual_key(Code::KeyA).ok(), Some(0x00));
        assert_eq!(virtual_key(Code::Enter).ok(), Some(0x24));
        assert_eq!(virtual_key(Code::Space).ok(), Some(0x31));
        assert_eq!(virtual_key(Code::ArrowUp).ok(), Some(0x7E));
        assert!(matches!(
            virtual_key(Code::BrowserBack),
            Err(Error::UnmappableKey(Code::BrowserBack))
        ));
    }

    #[test]
    fn modifiers_map_to_event_flags() {
        assert_eq!(
            event_flags(Modifiers::META | Modifiers::SHIFT).ok(),
            Some(CGEventFlags::MaskCommand | CGEventFlags::MaskShift)
        );
        assert_eq!(
            event_flags(Modifiers::empty()).ok(),
            Some(CGEventFlags::empty())
        );
    }

    #[test]
    fn a_modifier_without_a_flag_is_reported() {
        assert!(matches!(
            event_flags(Modifiers::CONTROL | Modifiers::SCROLL_LOCK),
            Err(Error::UnmappableModifiers(unmapped)) if unmapped == Modifiers::SCROLL_LOCK
        ));
    }
}
