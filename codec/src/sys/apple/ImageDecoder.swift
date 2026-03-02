import Foundation
import CoreGraphics
import ImageIO

private func invalidDecodedImage() -> SwiftDecodedImage {
    SwiftDecodedImage(width: 0, height: 0, pixels: RustVec(), is_valid: false, hdr: false)
}

public func decode_heif_image(data: RustVec<UInt8>) -> SwiftDecodedImage {
    let count = Int(data.len())
    if count == 0 {
        return invalidDecodedImage()
    }

    var encoded = Data(capacity: count)
    for i in 0..<count {
        guard let byte = data.get(index: UInt(i)) else {
            return invalidDecodedImage()
        }
        encoded.append(byte)
    }

    guard let source = CGImageSourceCreateWithData(encoded as CFData, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        return invalidDecodedImage()
    }

    let width = image.width
    let height = image.height
    if width == 0 || height == 0 {
        return invalidDecodedImage()
    }

    let bytesPerRow = width * 4
    var rgba = [UInt8](repeating: 0, count: width * height * 4)

    let colorSpace = CGColorSpaceCreateDeviceRGB()
    let bitmapInfo = CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
    guard let context = CGContext(data: &rgba,
                                  width: width,
                                  height: height,
                                  bitsPerComponent: 8,
                                  bytesPerRow: bytesPerRow,
                                  space: colorSpace,
                                  bitmapInfo: bitmapInfo) else {
        return invalidDecodedImage()
    }

    context.draw(image, in: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)))

    let pixels = RustVec<UInt8>()
    for byte in rgba {
        pixels.push(value: byte)
    }

    return SwiftDecodedImage(
        width: UInt32(width),
        height: UInt32(height),
        pixels: pixels,
        is_valid: true,
        hdr: false
    )
}
