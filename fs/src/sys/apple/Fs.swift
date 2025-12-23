import Foundation

#if os(iOS)
import UIKit
#endif

public func documents_dir() -> String? {
    return FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first?.path
}

public func cache_dir() -> String? {
    return FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?.path
}
