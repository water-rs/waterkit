import AuthenticationServices
import Foundation
import ObjectiveC

#if os(iOS)
import UIKit
#endif

#if os(macOS)
import AppKit
#endif

private struct RegisterRequest: Decodable {
    let rp_id: String
    let rp_name: String
    let user_name: String
    let user_display_name: String
    let user_id_b64u: String
    let challenge_b64u: String
    let timeout_ms: UInt32?
    let attestation: String
    let user_verification: String
    let discoverable: Bool
    let algorithms: [Int32]
    let exclude_credentials: [String]
}

private struct AuthenticateRequest: Decodable {
    let rp_id: String
    let challenge_b64u: String
    let timeout_ms: UInt32?
    let user_verification: String
    let allow_credentials: [String]
}

private var callbackAssociationKey: UInt8 = 0

private extension Data {
    init?(base64URLString: String) {
        var base64 = base64URLString.replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        let remainder = base64.count % 4
        if remainder > 0 {
            base64 += String(repeating: "=", count: 4 - remainder)
        }

        guard let data = Data(base64Encoded: base64) else {
            return nil
        }

        self = data
    }

    func base64URLString() -> String {
        return self.base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

private func parseRegisterRequest(_ requestJSON: RustStr) -> RegisterRequest? {
    let data = requestJSON.toString().data(using: .utf8)
    guard let payload = data else {
        return nil
    }

    return try? JSONDecoder().decode(RegisterRequest.self, from: payload)
}

private func parseAuthenticateRequest(_ requestJSON: RustStr) -> AuthenticateRequest? {
    let data = requestJSON.toString().data(using: .utf8)
    guard let payload = data else {
        return nil
    }

    return try? JSONDecoder().decode(AuthenticateRequest.self, from: payload)
}

private func serializePayload(_ payload: [String: Any]) -> String? {
    guard JSONSerialization.isValidJSONObject(payload) else {
        return nil
    }

    guard let data = try? JSONSerialization.data(withJSONObject: payload) else {
        return nil
    }

    return String(data: data, encoding: .utf8)
}

@available(iOS 15.0, macOS 12.0, *)
private func userVerificationPreference(
    from value: String
) -> ASAuthorizationPublicKeyCredentialUserVerificationPreference {
    switch value {
    case "discouraged":
        return .discouraged
    case "preferred":
        return .preferred
    default:
        return .required
    }
}

@available(iOS 16.0, macOS 13.0, *)
private func attestationPreference(
    from value: String
) -> ASAuthorizationPublicKeyCredentialAttestationKind {
    switch value {
    case "direct":
        return .direct
    case "indirect":
        return .indirect
    case "enterprise":
        return .enterprise
    default:
        return .none
    }
}

@available(iOS 15.0, macOS 12.0, *)
private final class RegisterControllerDelegate: NSObject,
    ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding {
    private let callback: RegisterCallback

    init(callback: RegisterCallback) {
        self.callback = callback
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        guard let credential = authorization.credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration else {
            callback.on_register_error("unexpected passkey registration credential type")
            return
        }

        var payload: [String: Any] = [:]
        payload["credential_id_b64u"] = credential.credentialID.base64URLString()
        guard let attestationObject = credential.rawAttestationObject else {
            callback.on_register_error("registration attestation object is missing")
            return
        }
        payload["attestation_object_b64u"] = attestationObject.base64URLString()
        payload["client_data_json_b64u"] = credential.rawClientDataJSON.base64URLString()

        guard let payloadString = serializePayload(payload) else {
            callback.on_register_error("failed to serialize passkey registration payload")
            return
        }

        callback.on_register_success(payloadString)
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithError error: Error
    ) {
        callback.on_register_error(error.localizedDescription)
    }

    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        return resolvePresentationAnchor()
    }
}

@available(iOS 15.0, macOS 12.0, *)
private final class AuthenticateControllerDelegate: NSObject,
    ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding {
    private let callback: AuthenticateCallback

    init(callback: AuthenticateCallback) {
        self.callback = callback
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        guard let credential = authorization.credential as? ASAuthorizationPlatformPublicKeyCredentialAssertion else {
            callback.on_authenticate_error("unexpected passkey assertion credential type")
            return
        }

        var payload: [String: Any] = [:]
        payload["credential_id_b64u"] = credential.credentialID.base64URLString()
        payload["authenticator_data_b64u"] = credential.rawAuthenticatorData.base64URLString()
        payload["client_data_json_b64u"] = credential.rawClientDataJSON.base64URLString()
        payload["signature_b64u"] = credential.signature.base64URLString()
        if let userID = credential.userID {
            payload["user_handle_b64u"] = userID.base64URLString()
        }

        guard let payloadString = serializePayload(payload) else {
            callback.on_authenticate_error("failed to serialize passkey authentication payload")
            return
        }

        callback.on_authenticate_success(payloadString)
    }

    func authorizationController(
        controller: ASAuthorizationController,
        didCompleteWithError error: Error
    ) {
        callback.on_authenticate_error(error.localizedDescription)
    }

    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        return resolvePresentationAnchor()
    }
}

private func runOnMain(_ block: @escaping () -> Void) {
    if Thread.isMainThread {
        block()
    } else {
        DispatchQueue.main.async(execute: block)
    }
}

@available(iOS 15.0, macOS 12.0, *)
private func perform(
    request: ASAuthorizationRequest,
    delegate: NSObject & ASAuthorizationControllerDelegate & ASAuthorizationControllerPresentationContextProviding
) {
    let controller = ASAuthorizationController(authorizationRequests: [request])
    controller.delegate = delegate
    controller.presentationContextProvider = delegate
    objc_setAssociatedObject(
        controller,
        &callbackAssociationKey,
        delegate,
        objc_AssociationPolicy.OBJC_ASSOCIATION_RETAIN_NONATOMIC
    )
    controller.performRequests()
}

public func passkey_is_available() -> Bool {
    if #available(iOS 16.0, macOS 13.0, *) {
        return true
    }

    return false
}

public func passkey_register(request_json: RustStr, callback: RegisterCallback) {
    guard #available(iOS 16.0, macOS 13.0, *) else {
        callback.on_register_error("passkey registration requires iOS 16+/macOS 13+")
        return
    }

    guard let request = parseRegisterRequest(request_json) else {
        callback.on_register_error("invalid registration request payload")
        return
    }

    guard let challenge = Data(base64URLString: request.challenge_b64u) else {
        callback.on_register_error("invalid challenge encoding")
        return
    }

    guard let userID = Data(base64URLString: request.user_id_b64u) else {
        callback.on_register_error("invalid user id encoding")
        return
    }

    runOnMain {
        let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(
            relyingPartyIdentifier: request.rp_id
        )
        let registrationRequest = provider.createCredentialRegistrationRequest(
            challenge: challenge,
            name: request.user_name,
            userID: userID
        )
        registrationRequest.displayName = request.user_display_name
        registrationRequest.userVerificationPreference = userVerificationPreference(
            from: request.user_verification
        )

        registrationRequest.attestationPreference = attestationPreference(from: request.attestation)

        let delegate = RegisterControllerDelegate(callback: callback)
        perform(request: registrationRequest, delegate: delegate)
    }
}

public func passkey_authenticate(request_json: RustStr, callback: AuthenticateCallback) {
    guard #available(iOS 16.0, macOS 13.0, *) else {
        callback.on_authenticate_error("passkey authentication requires iOS 16+/macOS 13+")
        return
    }

    guard let request = parseAuthenticateRequest(request_json) else {
        callback.on_authenticate_error("invalid authentication request payload")
        return
    }

    guard let challenge = Data(base64URLString: request.challenge_b64u) else {
        callback.on_authenticate_error("invalid challenge encoding")
        return
    }

    runOnMain {
        let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(
            relyingPartyIdentifier: request.rp_id
        )
        let assertionRequest = provider.createCredentialAssertionRequest(challenge: challenge)
        assertionRequest.userVerificationPreference = userVerificationPreference(
            from: request.user_verification
        )

        if !request.allow_credentials.isEmpty {
            let descriptors: [ASAuthorizationPlatformPublicKeyCredentialDescriptor] =
                request.allow_credentials.compactMap { encoded in
                    guard let credentialID = Data(base64URLString: encoded) else {
                        return nil
                    }
                    return ASAuthorizationPlatformPublicKeyCredentialDescriptor(
                        credentialID: credentialID
                    )
                }

            if !descriptors.isEmpty {
                assertionRequest.allowedCredentials = descriptors
            }
        }

        let delegate = AuthenticateControllerDelegate(callback: callback)
        perform(request: assertionRequest, delegate: delegate)
    }
}

private func resolvePresentationAnchor() -> ASPresentationAnchor {
    #if os(iOS)
    for scene in UIApplication.shared.connectedScenes {
        guard let windowScene = scene as? UIWindowScene else {
            continue
        }

        if let keyWindow = windowScene.windows.first(where: { $0.isKeyWindow }) {
            return keyWindow
        }
    }

    return UIWindow(frame: .zero)
    #elseif os(macOS)
    if let keyWindow = NSApplication.shared.keyWindow {
        return keyWindow
    }

    if let firstWindow = NSApplication.shared.windows.first {
        return firstWindow
    }

    return NSWindow()
    #endif
}
