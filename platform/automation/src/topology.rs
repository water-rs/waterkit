//! `CoreGraphics`' view of the display topology in this process.
//!
//! The per-display predicates (`CGDisplayIsOnline` and friends) answer
//! `0xFFFFFFFF` for an id `CoreGraphics` does not know, which reads as true,
//! so membership in the display lists decides instead.

use core::ptr;

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayIsInMirrorSet, CGError, CGGetActiveDisplayList,
    CGGetOnlineDisplayList, kCGNullDirectDisplay,
};

use crate::Error;
use crate::error::platform;

/// Reads a display list through `list`, which has the shape of
/// `CGGetOnlineDisplayList`: maximum, list buffer, count out pointer.
pub fn read_list(
    name: &str,
    list: impl Fn(u32, *mut CGDirectDisplayID, *mut u32) -> CGError,
) -> Result<Vec<CGDirectDisplayID>, Error> {
    let mut count = 0_u32;
    // A null list with a zero maximum asks only for the count.
    let status = list(0, ptr::null_mut(), &raw mut count);
    if status != CGError::Success {
        return Err(platform(name, status));
    }
    let mut displays = vec![kCGNullDirectDisplay; count as usize];
    let status = list(count, displays.as_mut_ptr(), &raw mut count);
    if status != CGError::Success {
        return Err(platform(name, status));
    }
    displays.truncate(count as usize);
    Ok(displays)
}

/// The displays `CoreGraphics` reports online in this process.
pub fn online() -> Result<Vec<CGDirectDisplayID>, Error> {
    // SAFETY: `read_list` passes a buffer of at least `max` entries, or a null
    // buffer with a zero maximum, and a valid count pointer.
    read_list("CGGetOnlineDisplayList", |max, list, count| unsafe {
        CGGetOnlineDisplayList(max, list, count)
    })
}

/// The displays `CoreGraphics` reports active in this process.
pub fn active() -> Result<Vec<CGDirectDisplayID>, Error> {
    // SAFETY: as in `online`.
    read_list("CGGetActiveDisplayList", |max, list, count| unsafe {
        CGGetActiveDisplayList(max, list, count)
    })
}

/// Whether one display is online, active and mirrored, as this process sees
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    pub online: bool,
    pub active: bool,
    pub mirrored: bool,
}

impl Presence {
    pub fn of(id: CGDirectDisplayID) -> Result<Self, Error> {
        let online = online()?.contains(&id);
        Ok(Self {
            online,
            active: active()?.contains(&id),
            // Only a listed display answers the predicate meaningfully.
            mirrored: online && CGDisplayIsInMirrorSet(id),
        })
    }

    /// Online, active and outside any mirror set: a display windows can be
    /// put on.
    pub const fn ready(self) -> bool {
        self.online && self.active && !self.mirrored
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_graphics::CGMainDisplayID;

    use super::*;

    /// Reads the lists only; nothing changes.
    #[test]
    fn an_unknown_display_is_absent_and_the_main_one_present() {
        let unknown = Presence::of(u32::MAX).expect("the lists read");
        assert_eq!(
            unknown,
            Presence {
                online: false,
                active: false,
                mirrored: false
            }
        );
        assert!(
            Presence::of(CGMainDisplayID())
                .expect("the lists read")
                .online
        );
    }
}
