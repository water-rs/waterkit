/// A point normalized to the upright image, where `(0, 0)` is the top-left
/// and `(1, 1)` is the bottom-right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal position, normalized to the upright image.
    pub x: f32,
    /// Vertical position, normalized to the upright image.
    pub y: f32,
}

/// Four corners in reading order: top-left, top-right, bottom-right,
/// bottom-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad(pub [Point; 4]);
