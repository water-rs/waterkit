import Foundation
import CoreLocation

// Swift implementation using swift-bridge generated types

class LocationDelegate: NSObject, CLLocationManagerDelegate {
    var location: CLLocation?
    var error: Error?
    var completed = false
    
    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        location = locations.last
        completed = true
    }
    
    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        self.error = error
        completed = true
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
    
    manager.requestLocation()
    
    // Wait for result (with timeout)
    let timeout = Date().addingTimeInterval(10)
    while !delegate.completed && Date() < timeout {
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
    }
    
    if !delegate.completed {
        return .Timeout
    }
    
    guard let location = delegate.location else {
        return .NotAvailable
    }
    
    let timestampMs = UInt64(location.timestamp.timeIntervalSince1970 * 1000)
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
