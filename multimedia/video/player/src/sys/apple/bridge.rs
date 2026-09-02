//! Swift bridge marker for the Apple picture-in-picture helper.

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn waterkit_video_swift_bridge_marker() -> bool;
    }
}
