import Foundation
import CoreImage
import CoreGraphics
import ImageIO
import VideoToolbox

private let pixelFormatInvalid: UInt8 = 0
private let pixelFormatRgba8UnormSrgb: UInt8 = 1
private let pixelFormatRgba16Float: UInt8 = 2
private let hdrHeadroomThreshold: Float = 1.001

private func invalidDecodedImage() -> SwiftDecodedImage {
    SwiftDecodedImage(
        width: 0,
        height: 0,
        pixels: RustVec(),
        is_valid: false,
        hdr: false,
        pixel_format: pixelFormatInvalid
    )
}

public func av1_hardware_decode_supported() -> Bool {
    if #available(iOS 17.0, macOS 14.0, tvOS 17.0, visionOS 1.0, *) {
        return VTIsHardwareDecodeSupported(kCMVideoCodecType_AV1)
    }
    return false
}

private func rustVec(from bytes: [UInt8]) -> RustVec<UInt8> {
    let pixels = RustVec<UInt8>()
    for byte in bytes {
        pixels.push(value: byte)
    }
    return pixels
}

@inline(__always)
private func float16FromLittleEndian(_ low: UInt8, _ high: UInt8) -> Float {
    let bits = UInt16(low) | (UInt16(high) << 8)
    return Float(Float16(bitPattern: bits))
}

private func rgba16fHasHdrHeadroom(_ rgba16f: [UInt8]) -> Bool {
    guard rgba16f.count.isMultiple(of: 8) else {
        return false
    }
    var i = 0
    while i < rgba16f.count {
        let r = float16FromLittleEndian(rgba16f[i], rgba16f[i + 1])
        let g = float16FromLittleEndian(rgba16f[i + 2], rgba16f[i + 3])
        let b = float16FromLittleEndian(rgba16f[i + 4], rgba16f[i + 5])
        if r > hdrHeadroomThreshold || g > hdrHeadroomThreshold || b > hdrHeadroomThreshold {
            return true
        }
        i += 8
    }
    return false
}

@inline(__always)
private func linearToUnorm8(_ value: Float) -> UInt8 {
    let clamped = min(max(value, 0.0), 1.0)
    return UInt8((clamped * 255.0).rounded())
}

@inline(__always)
private func linearToSrgbUnorm8(_ value: Float) -> UInt8 {
    let clamped = min(max(value, 0.0), 1.0)
    let encoded = clamped <= 0.003_130_8
        ? clamped * 12.92
        : 1.055 * pow(clamped, 1.0 / 2.4) - 0.055
    return UInt8((encoded * 255.0).rounded())
}

private func rgba16fToRgba8(_ rgba16f: [UInt8]) -> [UInt8]? {
    guard rgba16f.count.isMultiple(of: 8) else {
        return nil
    }
    var rgba8 = [UInt8](repeating: 0, count: rgba16f.count / 2)
    var src = 0
    var dst = 0
    while src < rgba16f.count {
        let r = float16FromLittleEndian(rgba16f[src], rgba16f[src + 1])
        let g = float16FromLittleEndian(rgba16f[src + 2], rgba16f[src + 3])
        let b = float16FromLittleEndian(rgba16f[src + 4], rgba16f[src + 5])
        let a = float16FromLittleEndian(rgba16f[src + 6], rgba16f[src + 7])
        rgba8[dst] = linearToSrgbUnorm8(r)
        rgba8[dst + 1] = linearToSrgbUnorm8(g)
        rgba8[dst + 2] = linearToSrgbUnorm8(b)
        rgba8[dst + 3] = linearToUnorm8(a)
        src += 8
        dst += 4
    }
    return rgba8
}

public func decode_isobmff_image(data: RustVec<UInt8>) -> SwiftDecodedImage {
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

    guard let source = CGImageSourceCreateWithData(encoded as CFData, nil) else {
        return invalidDecodedImage()
    }
    guard CGImageSourceCreateImageAtIndex(source, 0, nil) != nil else {
        return invalidDecodedImage()
    }

    guard let ciImage = CIImage(data: encoded) else {
        return invalidDecodedImage()
    }

    let extent = ciImage.extent.integral
    let width = Int(extent.width)
    let height = Int(extent.height)
    if width <= 0 || height <= 0 {
        return invalidDecodedImage()
    }
    if width > Int(UInt32.max) || height > Int(UInt32.max) {
        return invalidDecodedImage()
    }

    let (bytesPerRow, rowOverflow) = width.multipliedReportingOverflow(by: 8)
    if rowOverflow || bytesPerRow <= 0 {
        return invalidDecodedImage()
    }
    let (pixelCount, pixelCountOverflow) = width.multipliedReportingOverflow(by: height)
    if pixelCountOverflow {
        return invalidDecodedImage()
    }
    let (byteCount, byteCountOverflow) = pixelCount.multipliedReportingOverflow(by: 8)
    if byteCountOverflow || byteCount <= 0 {
        return invalidDecodedImage()
    }

    var rgba16f = [UInt8](repeating: 0, count: byteCount)
    guard let colorSpace = CGColorSpace(name: CGColorSpace.extendedLinearSRGB) else {
        return invalidDecodedImage()
    }

    let context = CIContext(options: [
        CIContextOption.workingColorSpace: colorSpace,
        CIContextOption.outputColorSpace: colorSpace
    ])
    context.render(
        ciImage,
        toBitmap: &rgba16f,
        rowBytes: bytesPerRow,
        bounds: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)),
        format: .RGBAh,
        colorSpace: colorSpace
    )

    let hdr = rgba16fHasHdrHeadroom(rgba16f)
    if hdr {
        return SwiftDecodedImage(
            width: UInt32(width),
            height: UInt32(height),
            pixels: rustVec(from: rgba16f),
            is_valid: true,
            hdr: true,
            pixel_format: pixelFormatRgba16Float
        )
    }

    guard let rgba8 = rgba16fToRgba8(rgba16f) else {
        return invalidDecodedImage()
    }

    return SwiftDecodedImage(
        width: UInt32(width),
        height: UInt32(height),
        pixels: rustVec(from: rgba8),
        is_valid: true,
        hdr: false,
        pixel_format: pixelFormatRgba8UnormSrgb
    )
}
