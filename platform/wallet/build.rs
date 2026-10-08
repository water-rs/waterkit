//! Build script for waterkit-wallet.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Wallet.swift")
                    .framework("Foundation")
                    .framework("PassKit")
                    .framework("UIKit"),
            )
            .compile();
    }
}
