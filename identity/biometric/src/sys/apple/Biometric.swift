import LocalAuthentication

public func biometric_is_available() -> Bool {
    let context = LAContext()
    var error: NSError?
    return context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
}

public func biometric_get_type() -> UInt8 {
    let context = LAContext()
    var error: NSError?
    if context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) {
        switch context.biometryType {
        case .touchID:
            return 1
        case .faceID:
            return 2
        case .opticID:
            return 3
        default:
            return 0
        }
    }
    return 0
}

public func biometric_authenticate(reason: RustStr, callback: BiometricCallback) {
    let context = LAContext()
    let reasonStr = reason.toString()

    context.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, localizedReason: reasonStr) { success, authenticationError in
        if success {
            callback.on_success()
        } else {
            let errorMessage = authenticationError?.localizedDescription ?? "Unknown error"
            callback.on_error(errorMessage)
        }
    }
}
