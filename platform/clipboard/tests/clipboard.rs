//! Linux CLIPBOARD tests.
//!
//! The round trip needs a display server and that display server's clipboard
//! CLI, which it uses to check CLIPBOARD from outside the crate:
//!
//! - X11: `DISPLAY` pointing at an X server and `xclip` installed. On a
//!   machine without a desktop session, run the suite under `xvfb-run`
//!   (package `xvfb`); CI starts an Xvfb server for it.
//! - Wayland: `WAYLAND_DISPLAY` pointing at a compositor that offers a
//!   data-control protocol, and `wl-clipboard` installed; CI runs it against
//!   headless sway.
//!
//! A missing display server or tool fails the test with a message naming
//! what to install; it never passes without exercising CLIPBOARD. Every
//! format is checked in one test, because the formats share the one
//! CLIPBOARD and the test runner runs tests in parallel.
//!
//! The ignored test checks that a Wayland session without data-control never
//! falls through to X11. It needs `WAYLAND_DISPLAY` at a compositor without a
//! data-control protocol (headless weston) and `DISPLAY` at an X server; CI
//! runs it with `--run-ignored only` against those two.
#![cfg(target_os = "linux")]

mod common;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::External;
use futures::StreamExt as _;
use futures::executor::block_on;
use image::{ImageFormat, RgbaImage};
use waterkit_clipboard::{Clipboard, ClipboardError, ClipboardEvent, Image};

const TEXT: &str = "waterkit owned clipboard \u{2713}";
const EXTERNAL_TEXT: &str = "waterkit external clipboard";
const HTML: &str = "<b>waterkit</b> owned";
const ALT_TEXT: &str = "waterkit owned";
const EXTERNAL_HTML: &str = "<i>waterkit</i> external";
const CUSTOM_MIME: &str = "application/x-waterkit-test";
const CUSTOM: &[u8] = &[0, 1, 2, 254, 255];
const EXTERNAL_CUSTOM: &[u8] = &[9, 8, 7];

/// How long a watch may take to report a change made by the external tool.
const WATCH_DEADLINE: Duration = Duration::from_secs(5);

/// A size above every display server's limit for one transfer (on X11 a
/// quarter of the maximum request size), so the data moves incrementally.
const LARGE_SIZE: usize = 5 * 1024 * 1024 + 7;

/// One step of the round trip.
type Step = fn(&External, &mut Clipboard) -> Result<(), ClipboardError>;

/// Write every format through the crate and read it back through the crate
/// and through the display server's clipboard tool; claim CLIPBOARD with each
/// format through that tool and read it through the crate; watch a change;
/// clear. A failure names the step it happened in.
#[test]
fn clipboard_round_trip() -> Result<(), String> {
    let external = External::detect("CLIPBOARD");
    let mut clipboard = Clipboard::new().map_err(|error| format!("Clipboard::new: {error:?}"))?;
    let steps: [(&str, Step); 8] = [
        ("text", text),
        ("html", html),
        ("files", files),
        ("image", image),
        ("custom", custom),
        ("large", large),
        ("watch", watch),
        ("clear", clear),
    ];
    for (name, step) in steps {
        step(&external, &mut clipboard).map_err(|error| format!("{name} step: {error:?}"))?;
    }
    Ok(())
}

fn text(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    clipboard.set_text(TEXT)?;
    assert!(clipboard.has_text()?);
    assert!(!clipboard.has_html()?);
    assert_eq!(block_on(clipboard.text())?.as_deref(), Some(TEXT));
    assert_eq!(external.read(None), TEXT.as_bytes());

    external.write(EXTERNAL_TEXT.as_bytes(), None);
    let read = common::read_until(&Some(EXTERNAL_TEXT.to_owned()), || {
        block_on(clipboard.text())
    })?;
    assert_eq!(read.as_deref(), Some(EXTERNAL_TEXT));
    Ok(())
}

fn html(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    clipboard.set_html(HTML, Some(ALT_TEXT))?;
    assert!(clipboard.has_html()?);
    assert_eq!(block_on(clipboard.html())?.as_deref(), Some(HTML));
    assert_eq!(block_on(clipboard.text())?.as_deref(), Some(ALT_TEXT));
    assert_eq!(external.read(Some("text/html")), HTML.as_bytes());
    assert_eq!(external.read(None), ALT_TEXT.as_bytes());

    clipboard.set_html(HTML, None)?;
    assert!(clipboard.has_html()?);
    assert!(
        !clipboard.has_text()?,
        "HTML without alt text is offered as plain text"
    );

    external.write(EXTERNAL_HTML.as_bytes(), Some("text/html"));
    let read = common::read_until(&Some(EXTERNAL_HTML.to_owned()), || {
        block_on(clipboard.html())
    })?;
    assert_eq!(read.as_deref(), Some(EXTERNAL_HTML));
    Ok(())
}

fn files(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    let owned = vec![
        PathBuf::from("/tmp/waterkit clipboard/owned.txt"),
        PathBuf::from("/tmp/waterkit-\u{e9}.txt"),
    ];
    clipboard.set_files(&owned)?;
    assert!(clipboard.has_files()?);
    assert_eq!(block_on(clipboard.files())?, owned);
    assert_eq!(
        external.read(Some("text/uri-list")),
        b"file:///tmp/waterkit%20clipboard/owned.txt\r\nfile:///tmp/waterkit-%C3%A9.txt\r\n"
    );

    external.write(
        b"file:///tmp/waterkit%20external.txt\r\n",
        Some("text/uri-list"),
    );
    let expected = vec![PathBuf::from("/tmp/waterkit external.txt")];
    let read = common::read_until(&expected, || block_on(clipboard.files()))?;
    assert_eq!(read, expected);
    Ok(())
}

/// The size and RGBA pixels of an image, comparable.
fn pixels(image: &Image) -> (u32, u32, Vec<u8>) {
    (image.width(), image.height(), image.bytes().to_vec())
}

fn rgba(width: u32, height: u32, pixels: &[u8]) -> RgbaImage {
    RgbaImage::from_raw(width, height, pixels.to_vec()).expect("pixels match the size")
}

fn png(image: &RgbaImage) -> Vec<u8> {
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .expect("failed to encode the test image");
    png
}

/// A PNG file holding `image`, removed when dropped.
struct PngFile(PathBuf);

impl PngFile {
    fn new(image: &RgbaImage) -> Self {
        let path = std::env::temp_dir().join(format!(
            "waterkit-clipboard-test-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, png(image)).expect("failed to write the test image");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PngFile {
    fn drop(&mut self) {
        // Leaving the file behind would only litter the temp directory.
        let _ = std::fs::remove_file(&self.0);
    }
}

fn image(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    let owned = rgba(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
    let file = PngFile::new(&owned);
    clipboard.set_image(file.path())?;
    assert!(clipboard.has_image()?);
    let read = block_on(clipboard.image())?.expect("the image just written");
    assert_eq!(pixels(&read), (2, 1, owned.to_vec()));
    let external_png = external.read(Some("image/png"));
    let decoded = image::load_from_memory_with_format(&external_png, ImageFormat::Png)
        .expect("the external tool read no PNG")
        .to_rgba8();
    assert_eq!(decoded, owned);

    let seeded = rgba(1, 2, &[0, 255, 0, 255, 10, 20, 30, 40]);
    external.write(&png(&seeded), Some("image/png"));
    let expected = Some((1, 2, seeded.to_vec()));
    let read = common::read_until(&expected, || {
        block_on(clipboard.image()).map(|image| image.as_ref().map(pixels))
    })?;
    assert_eq!(read, expected);
    Ok(())
}

fn custom(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    clipboard.set_binary(CUSTOM, CUSTOM_MIME)?;
    assert_eq!(
        block_on(clipboard.binary(CUSTOM_MIME))?.as_deref(),
        Some(CUSTOM)
    );
    assert_eq!(block_on(clipboard.binary("application/x-absent"))?, None);
    assert_eq!(external.read(Some(CUSTOM_MIME)), CUSTOM);

    external.write(EXTERNAL_CUSTOM, Some(CUSTOM_MIME));
    let expected = Some(EXTERNAL_CUSTOM.to_vec());
    let read = common::read_until(&expected, || block_on(clipboard.binary(CUSTOM_MIME)))?;
    assert_eq!(read, expected);
    Ok(())
}

/// [`LARGE_SIZE`] bytes counting up to `period` over and over; a period that
/// does not divide the size catches a dropped or repeated increment.
fn large_payload(period: u8) -> Vec<u8> {
    (0..period).cycle().take(LARGE_SIZE).collect()
}

/// A payload too large for one transfer, in both directions.
fn large(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    let owned = large_payload(251);
    clipboard.set_binary(&owned, CUSTOM_MIME)?;
    assert!(
        block_on(clipboard.binary(CUSTOM_MIME))?.as_deref() == Some(owned.as_slice()),
        "the crate read back different bytes than it wrote"
    );
    assert!(
        external.read(Some(CUSTOM_MIME)) == owned,
        "the external tool read different bytes than the crate wrote"
    );

    let seeded = large_payload(239);
    external.write(&seeded, Some(CUSTOM_MIME));
    let expected = Some(seeded);
    let read = common::read_until(&expected, || block_on(clipboard.binary(CUSTOM_MIME)))?;
    assert!(
        read == expected,
        "the crate read different bytes than the external tool wrote"
    );
    Ok(())
}

fn clear(_: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    clipboard.clear()?;
    assert!(!clipboard.has_text()?);
    assert_eq!(block_on(clipboard.text())?, None);
    assert_eq!(block_on(clipboard.files())?, Vec::<PathBuf>::new());
    Ok(())
}

/// A watch reports a change made after it started, and not the content held
/// when it started.
fn watch(external: &External, clipboard: &mut Clipboard) -> Result<(), ClipboardError> {
    clipboard.set_text(TEXT)?;
    let mut stream = clipboard.watch()?;
    external.write(EXTERNAL_HTML.as_bytes(), Some("text/html"));

    let (sender, receiver) = mpsc::channel::<Option<ClipboardEvent>>();
    std::thread::spawn(move || {
        // A send fails only when the test already timed out.
        let _ = sender.send(block_on(stream.next()));
    });
    let event = receiver
        .recv_timeout(WATCH_DEADLINE)
        .unwrap_or_else(|_| panic!("the watch reported no change within {WATCH_DEADLINE:?}"))
        .expect("the watch stream ended");
    assert!(
        event.has_html(),
        "the first event is not the external HTML change: {event:?}"
    );
    Ok(())
}

/// In a Wayland session whose compositor offers no data-control protocol,
/// creating the handle fails with the documented error even though an X
/// server is reachable through `DISPLAY`.
#[test]
#[ignore = "needs WAYLAND_DISPLAY at a compositor without data-control and DISPLAY at an X server"]
fn wayland_without_data_control_never_uses_x11() {
    common::assert_wayland_without_data_control_fails("CLIPBOARD", Clipboard::new());
}
