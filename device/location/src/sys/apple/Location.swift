import Foundation
import CoreLocation

private let locationRequestTimeout: TimeInterval = 10.0

private final class LocationRequest: NSObject, CLLocationManagerDelegate {
    private let manager = CLLocationManager()
    private var callback: ((LocationResult) -> Void)?
    private var timeout: DispatchWorkItem?
    private var keepAlive: LocationRequest?

    init(callback: @escaping (LocationResult) -> Void) {
        self.callback = callback
    }

    func start() {
        keepAlive = self
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let servicesEnabled = CLLocationManager.locationServicesEnabled()
            DispatchQueue.main.async { [weak self] in
                self?.startOnMain(servicesEnabled: servicesEnabled)
            }
        }
    }

    private func startOnMain(servicesEnabled: Bool) {
        guard servicesEnabled else {
            finish(.ServiceDisabled)
            return
        }
        let status = manager.authorizationStatus
        guard status != .denied && status != .restricted else {
            finish(.PermissionDenied)
            return
        }

        manager.delegate = self
        manager.desiredAccuracy = kCLLocationAccuracyBest
        let timeout = DispatchWorkItem { [weak self] in
            self?.finish(.Timeout)
        }
        self.timeout = timeout
        DispatchQueue.main.asyncAfter(
            deadline: .now() + locationRequestTimeout,
            execute: timeout
        )
        manager.requestLocation()
    }

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let location = locations.last else {
            finish(.NotAvailable)
            return
        }
        let timestampMs = Int64(location.timestamp.timeIntervalSince1970 * 1000)
        finish(.Success(LocationData(
            latitude: location.coordinate.latitude,
            longitude: location.coordinate.longitude,
            altitude: location.altitude,
            horizontal_accuracy: location.horizontalAccuracy,
            vertical_accuracy: location.verticalAccuracy,
            timestamp_ms: timestampMs
        )))
    }

    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        guard let code = (error as? CLError)?.code else {
            finish(.NotAvailable)
            return
        }
        switch code {
        case .denied:
            finish(.PermissionDenied)
        case .locationUnknown:
            break
        default:
            finish(.NotAvailable)
        }
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let status = manager.authorizationStatus
        if status == .denied || status == .restricted {
            finish(.PermissionDenied)
        }
    }

    private func finish(_ result: LocationResult) {
        guard let callback else {
            return
        }
        self.callback = nil
        timeout?.cancel()
        timeout = nil
        manager.delegate = nil
        callback(result)
        keepAlive = nil
    }
}

func get_current_location(callback: @escaping (LocationResult) -> Void) {
    DispatchQueue.main.async {
        LocationRequest(callback: callback).start()
    }
}
