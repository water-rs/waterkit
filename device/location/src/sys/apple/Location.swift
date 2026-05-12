import Foundation
import CoreLocation

// Swift implementation using swift-bridge generated types

class LocationDelegate: NSObject, CLLocationManagerDelegate {
    var location: CLLocation?
    var error: Error?
    var completed = false
    var runLoop: CFRunLoop?

    private func finish() {
        completed = true
        if let runLoop {
            CFRunLoopStop(runLoop)
        }
    }
    
    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        location = locations.last
        finish()
    }
    
    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        self.error = error
        finish()
    }
}

func get_current_location() -> LocationResult {
    // Check authorization
    let status = CLLocationManager.authorizationStatus()
    switch status {
    case .denied, .restricted:
        return .PermissionDenied
    case .notDetermined:
        return .PermissionDenied
    default:
        break
    }
    
    // Check if location services are enabled
    guard CLLocationManager.locationServicesEnabled() else {
        return .ServiceDisabled
    }
    
    let manager = CLLocationManager()
    let delegate = LocationDelegate()
    manager.delegate = delegate
    manager.desiredAccuracy = kCLLocationAccuracyBest
    delegate.runLoop = CFRunLoopGetCurrent()
    
    manager.requestLocation()
    
    if !delegate.completed {
        _ = CFRunLoopRunInMode(.defaultMode, 10.0, false)
    }
    
    if !delegate.completed {
        return .Timeout
    }
    
    guard let location = delegate.location else {
        return .NotAvailable
    }
    
    let timestampMs = Int64(location.timestamp.timeIntervalSince1970 * 1000)
    let data = LocationData(
        latitude: location.coordinate.latitude,
        longitude: location.coordinate.longitude,
        altitude: location.altitude,
        horizontal_accuracy: location.horizontalAccuracy,
        vertical_accuracy: location.verticalAccuracy,
        timestamp_ms: timestampMs
    )
    
    return .Success(data)
}
