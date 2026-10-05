import LocalAuthentication

/// Face ID, Touch ID or the passcode: what `.deviceOwnerAuthentication` will ask for here.
enum DeviceOwner {
    static var methodName: String {
        let context = LAContext()
        _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
        switch context.biometryType {
        case .faceID: return "Face ID"
        case .touchID: return "Touch ID"
        case .opticID: return "Optic ID"
        default: return "passcode"
        }
    }

    /// Asks the owner to prove it's them (biometrics, falling back to the passcode). Fails with
    /// a sentence for the user when they can't or won't.
    static func confirm(_ reason: String) async -> Result<Void, DeviceOwnerError> {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            return .failure(DeviceOwnerError(message: "Set a passcode on this iPhone to allow requests for a whole session."))
        }
        do {
            try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)
            return .success(())
        } catch let e as LAError where [.userCancel, .appCancel, .systemCancel].contains(e.code) {
            return .failure(DeviceOwnerError(message: nil))
        } catch {
            return .failure(DeviceOwnerError(message: "Couldn't confirm it's you. Nothing was sent."))
        }
    }
}

struct DeviceOwnerError: Error {
    /// Nil when the user cancelled (nothing to say).
    var message: String?
}
