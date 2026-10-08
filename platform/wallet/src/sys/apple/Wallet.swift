import Foundation
import PassKit
import UIKit

private var retainedDelegates: [ObjectIdentifier: WalletAddPassesDelegate] = [:]

private func retain(_ delegate: WalletAddPassesDelegate) {
    retainedDelegates[ObjectIdentifier(delegate)] = delegate
}

private func releaseDelegate(_ delegate: WalletAddPassesDelegate) {
    retainedDelegates.removeValue(forKey: ObjectIdentifier(delegate))
}

private func foregroundTopViewController() -> UIViewController? {
    let keyWindow = UIApplication.shared.connectedScenes
        .compactMap { $0 as? UIWindowScene }
        .first(where: { $0.activationState == .foregroundActive })?
        .windows
        .first(where: { $0.isKeyWindow })

    var top = keyWindow?.rootViewController
    while let presented = top?.presentedViewController {
        top = presented
    }
    return top
}

private final class WalletAddPassesDelegate: NSObject, PKAddPassesViewControllerDelegate {
    private let passes: [PKPass]
    private var callback: AddCallback?

    init(passes: [PKPass], callback: AddCallback) {
        self.passes = passes
        self.callback = callback
    }

    func addPassesViewControllerDidFinish(_ controller: PKAddPassesViewController) {
        controller.dismiss(animated: true) { [self] in
            guard let callback else {
                fatalError("waterkit-wallet: add-pass callback was already completed")
            }
            self.callback = nil
            let library = PKPassLibrary()
            if passes.allSatisfy({ library.containsPass($0) }) {
                callback.on_added()
            } else {
                callback.on_cancelled()
            }
            releaseDelegate(self)
        }
    }
}

private func presentReview(
    passes: [PKPass],
    from viewController: UIViewController,
    callback: AddCallback
) {
    let controller: PKAddPassesViewController?
    if passes.count == 1, let pass = passes.first {
        controller = PKAddPassesViewController(pass: pass)
    } else {
        controller = PKAddPassesViewController(passes: passes)
    }
    guard let controller else {
        callback.on_unavailable()
        return
    }

    let delegate = WalletAddPassesDelegate(passes: passes, callback: callback)
    controller.delegate = delegate
    retain(delegate)
    viewController.present(controller, animated: true)
}

func wallet_is_available() -> Bool {
    PKPassLibrary.isPassLibraryAvailable() && PKAddPassesViewController.canAddPasses()
}

func wallet_add(request: AddRequest, callback: AddCallback) {
    var passes = [PKPass]()
    for index in 0..<request.pass_count() {
        let data = Data(request.pass_data(index))
        do {
            passes.append(try PKPass(data: data))
        } catch {
            callback.on_invalid_pass(index, error.localizedDescription)
            return
        }
    }

    DispatchQueue.main.async {
        guard wallet_is_available() else {
            callback.on_unavailable()
            return
        }
        guard let viewController = foregroundTopViewController() else {
            callback.on_error("no foreground window to present Wallet from")
            return
        }

        if passes.count == 1 {
            presentReview(passes: passes, from: viewController, callback: callback)
            return
        }

        PKPassLibrary().addPasses(passes) { status in
            switch status {
            case .didAddPasses:
                callback.on_added()
            case .didCancelAddPasses:
                callback.on_cancelled()
            case .shouldReviewPasses:
                DispatchQueue.main.async {
                    presentReview(passes: passes, from: viewController, callback: callback)
                }
            @unknown default:
                callback.on_error("unknown PassKit add-pass status \(status.rawValue)")
            }
        }
    }
}
