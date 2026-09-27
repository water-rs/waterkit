#!/usr/bin/env bash
# Regenerate src/shaders/compiled from src/yuv_to_rgba.wgsl.
# Requires the shaderloom CLI (`cargo install --locked --git
# https://github.com/water-rs/shaderloom --rev <PIN> --features build`, where
# <PIN> is the [patch.crates-io] rev in the workspace Cargo.toml).
set -euo pipefail
cd "$(dirname "$0")"
shaderloom wgsl-package src/yuv_to_rgba.wgsl \
    --name yuv_color \
    --label src/yuv_to_rgba.wgsl \
    --out src/shaders/compiled
