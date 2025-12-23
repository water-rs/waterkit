# WaterUI Kit

High-level development kit and utility tools for the WaterUI framework.

## Overview

`waterui-kit` provides a comprehensive development kit with utilities, tools, and helper functions that make working with WaterUI more convenient and efficient. This crate includes development utilities, testing helpers, and common patterns.

## Features

- **Development Tools**: Utilities for debugging and development
- **Testing Helpers**: Test utilities for WaterUI components
- **Common Patterns**: Reusable UI patterns and compositions
- **Build Tools**: Integration with build systems and asset management

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
waterui-kit = "0.1.0"
```

## Development Utilities

The kit provides various utilities for development and debugging:

```rust
use waterui_kit::{debug, inspect, performance};

// Debug views in development
let debug_view = debug(my_view())
    .show_bounds(true)
    .show_layout_info(true);

// Performance monitoring
let monitored = performance::monitor(expensive_view())
    .track_render_time()
    .track_memory_usage();
```

## Testing Support

Helper functions for testing WaterUI components:

```rust
use waterui_kit::testing::*;

#[test]
fn test_my_component() {
    let component = MyComponent::new();
    let rendered = test_render(component);
    
    assert_eq!(rendered.text_content(), "Expected text");
    assert!(rendered.has_class("my-component"));
}
```

## Android Development Setup

When building for Android **outside of the WaterUI CLI** (e.g., running `cargo build --target aarch64-linux-android` directly), you need to set up the following environment variables:

### Required Environment Variables

```bash
# Android SDK path
export ANDROID_HOME="$HOME/Library/Android/sdk"          # macOS
export ANDROID_HOME="$HOME/Android/Sdk"                  # Linux
export ANDROID_SDK_ROOT="$ANDROID_HOME"

# Path to android.jar (use your installed API level)
export ANDROID_JAR="$ANDROID_HOME/platforms/android-34/android.jar"

# NDK configuration
export ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/<version>"   # e.g., ndk/29.0.14206865

# Compiler toolchain (for aarch64)
NDK_TOOLCHAIN="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/darwin-x86_64"  # macOS
NDK_TOOLCHAIN="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64"   # Linux

export CC_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$NDK_TOOLCHAIN/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang"
```

### Kotlin Compiler

The location and permission crates compile Kotlin code at build time. You need `kotlinc` available:

```bash
# Option 1: Install via Homebrew (recommended)
brew install kotlin

# Option 2: Use Android Studio's bundled Kotlin (may need chmod +x)
export PATH="/Applications/Android Studio.app/Contents/plugins/Kotlin/kotlinc/bin:$PATH"

# If Android Studio's kotlinc isn't executable:
sudo chmod +x "/Applications/Android Studio.app/Contents/plugins/Kotlin/kotlinc/bin/kotlinc"
```

### Quick Setup Script

```bash
#!/bin/bash
# Save as android-env.sh and run: source android-env.sh

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_ROOT="$(ls -d $ANDROID_HOME/ndk/*/ 2>/dev/null | sort -V | tail -1)"

# Find highest installed platform
PLATFORM=$(ls -d $ANDROID_HOME/platforms/android-*/ 2>/dev/null | sort -V | tail -1)
export ANDROID_JAR="$PLATFORM/android.jar"

# Set up NDK toolchain
NDK_TOOLCHAIN="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/darwin-x86_64"
export CC_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang"
export CXX_aarch64_linux_android="$NDK_TOOLCHAIN/bin/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$NDK_TOOLCHAIN/bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"

echo "ANDROID_HOME=$ANDROID_HOME"
echo "ANDROID_JAR=$ANDROID_JAR"
echo "ANDROID_NDK_ROOT=$ANDROID_NDK_ROOT"
```

### Using WaterUI CLI (Recommended)

The **WaterUI CLI** (`water run --platform android`) automatically configures all these environment variables. Use it when possible for the simplest development experience.

## Dependencies

- `waterui-core`: Core framework functionality

This crate is designed to be used during development and testing, and may not be needed in production builds.