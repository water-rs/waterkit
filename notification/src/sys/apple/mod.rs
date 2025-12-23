#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn show_notification(title: &str, body: &str);
    }
}

pub fn show_notification(title: &str, body: &str) {
    ffi::show_notification(title, body);
}
