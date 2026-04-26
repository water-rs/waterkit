import Foundation

#if os(macOS)
import IOKit
#else
import CoreMotion
#endif

// MARK: - Sensor Reading Helpers

private func currentTimestampMs() -> UInt64 {
    return UInt64(Date().timeIntervalSince1970 * 1000)
}

// MARK: - iOS Implementation

#if os(iOS)

private let motionManager = CMMotionManager()
private let sensorReadTimeout: DispatchTimeInterval = .seconds(1)

private func readMotionVectorSample<T>(
    startUpdates: (OperationQueue, @escaping (T?, Error?) -> Void) -> Void,
    stopUpdates: () -> Void,
    map: @escaping (T) -> SensorReading
) -> SensorResult {
    var result: SensorResult = .Timeout
    let semaphore = DispatchSemaphore(value: 0)
    let queue = OperationQueue()

    startUpdates(queue) { data, _ in
        if let data {
            result = .Success(map(data))
        }
        semaphore.signal()
    }

    if semaphore.wait(timeout: .now() + sensorReadTimeout) == .timedOut {
        result = .Timeout
    }

    stopUpdates()
    return result
}

func is_accelerometer_available() -> Bool {
    return motionManager.isAccelerometerAvailable
}

func read_accelerometer() -> SensorResult {
    guard motionManager.isAccelerometerAvailable else {
        return .NotAvailable
    }

    motionManager.accelerometerUpdateInterval = 0.01
    return readMotionVectorSample(
        startUpdates: motionManager.startAccelerometerUpdates(to:withHandler:),
        stopUpdates: motionManager.stopAccelerometerUpdates
    ) { data in
        SensorReading(
            x: data.acceleration.x,
            y: data.acceleration.y,
            z: data.acceleration.z,
            timestamp_ms: currentTimestampMs()
        )
    }
}

func is_gyroscope_available() -> Bool {
    return motionManager.isGyroAvailable
}

func read_gyroscope() -> SensorResult {
    guard motionManager.isGyroAvailable else {
        return .NotAvailable
    }

    motionManager.gyroUpdateInterval = 0.01
    return readMotionVectorSample(
        startUpdates: motionManager.startGyroUpdates(to:withHandler:),
        stopUpdates: motionManager.stopGyroUpdates
    ) { data in
        SensorReading(
            x: data.rotationRate.x,
            y: data.rotationRate.y,
            z: data.rotationRate.z,
            timestamp_ms: currentTimestampMs()
        )
    }
}

func is_magnetometer_available() -> Bool {
    return motionManager.isMagnetometerAvailable
}

func read_magnetometer() -> SensorResult {
    guard motionManager.isMagnetometerAvailable else {
        return .NotAvailable
    }

    motionManager.magnetometerUpdateInterval = 0.01
    return readMotionVectorSample(
        startUpdates: motionManager.startMagnetometerUpdates(to:withHandler:),
        stopUpdates: motionManager.stopMagnetometerUpdates
    ) { data in
        SensorReading(
            x: data.magneticField.x,
            y: data.magneticField.y,
            z: data.magneticField.z,
            timestamp_ms: currentTimestampMs()
        )
    }
}

func is_barometer_available() -> Bool {
    return CMAltimeter.isRelativeAltitudeAvailable()
}

func read_barometer() -> ScalarResult {
    guard CMAltimeter.isRelativeAltitudeAvailable() else {
        return .NotAvailable
    }
    
    let altimeter = CMAltimeter()
    var result: ScalarResult = .Timeout
    let semaphore = DispatchSemaphore(value: 0)
    let queue = OperationQueue()
    
    // Note: This API is "start updates", so we need to wait for the first update
    altimeter.startRelativeAltitudeUpdates(to: queue) { data, error in
        if let data = data {
            let reading = ScalarReading(
                value: data.pressure.doubleValue * 10.0, // kPa to hPa
                timestamp_ms: currentTimestampMs()
            )
            result = .Success(reading)
        }
        semaphore.signal()
    }
    
    if semaphore.wait(timeout: .now() + sensorReadTimeout) == .timedOut {
        result = .Timeout
    }
    
    altimeter.stopRelativeAltitudeUpdates()
    return result
}

// Ambient light is not exposed via public API on iOS
func is_ambient_light_available() -> Bool {
    return false
}

func read_ambient_light() -> ScalarResult {
    return .NotAvailable
}

#endif

// MARK: - macOS Implementation

#if os(macOS)

func is_accelerometer_available() -> Bool { return false }
func read_accelerometer() -> SensorResult { return .NotAvailable }

func is_gyroscope_available() -> Bool { return false }
func read_gyroscope() -> SensorResult { return .NotAvailable }

func is_magnetometer_available() -> Bool { return false }
func read_magnetometer() -> SensorResult { return .NotAvailable }

func is_barometer_available() -> Bool { return false }
func read_barometer() -> ScalarResult { return .NotAvailable }

// Ambient Light Support for macOS (IOKit)

func is_ambient_light_available() -> Bool {
    // Check for AppleLMUController
    let service = IOServiceGetMatchingService(kIOMasterPortDefault, IOServiceMatching("AppleLMUController"))
    if service != 0 {
        IOObjectRelease(service)
        return true
    }
    return false
}

func read_ambient_light() -> ScalarResult {
    let service = IOServiceGetMatchingService(kIOMasterPortDefault, IOServiceMatching("AppleLMUController"))
    guard service != 0 else {
        return .NotAvailable
    }
    defer { IOObjectRelease(service) }
    
    var conn: io_connect_t = 0
    let kr = IOServiceOpen(service, mach_task_self_, 0, &conn)
    guard kr == KERN_SUCCESS else {
        return .PermissionDenied
    }
    defer { IOServiceClose(conn) }
    
    var outputs: [UInt64] = [0, 0]
    var count: UInt32 = 2
    
    // Usage of AppleLMUController often involves selector 0 for reading values
    let result = IOConnectCallScalarMethod(conn, 0, nil, 0, &outputs, &count)
    if result != KERN_SUCCESS {
        return .NotAvailable
    }
    
    // Average the left/right sensors
    let avg = Double(outputs[0] + outputs[1]) / 2.0
    
    let reading = ScalarReading(
        value: avg,
        timestamp_ms: currentTimestampMs()
    )
    return .Success(reading)
}

#endif
