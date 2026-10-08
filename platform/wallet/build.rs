//! Build script for waterkit-wallet.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-wallet", "WalletHelper")
            .swift_source("src/sys/apple/Wallet.swift")
            .framework("Foundation")
            .framework("PassKit")
            .framework("UIKit");
        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }
}
