#!/bin/bash
# OpenPaw One-Step Installer for Linux/macOS
# Usage: curl -sSf https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.sh | bash

set -e

echo -e "\033[0;36m🐾 Starting OpenPaw Installation...\033[0m"

# 1. Check for Git
if ! command -v git &> /dev/null; then
    echo -e "\033[0;31m❌ Git not found. Please install git first.\033[0m"
    exit 1
fi

# 2. Check for Rust
if ! command -v rustc &> /dev/null; then
    echo -e "\033[0;33m🦀 Rust not found. Installing via rustup...\033[0m"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source $HOME/.cargo/env
fi

# 3. Clone if not in repo
if [ ! -f "Cargo.toml" ]; then
    echo -e "\033[0;36m📂 Cloning OpenPaw from GitHub...\033[0m"
    git clone https://github.com/deviprasadshetty-dev/openpaw.git
    cd openpaw
fi

# 4. Build OpenPaw
echo -e "\033[0;36m🏗️  Building OpenPaw (this may take a few minutes)...\033[0m"
cargo build --release

# 5. Check if build succeeded
if [ ! -f "./target/release/openpaw" ]; then
    echo -e "\033[0;31m❌ Build failed. Please check the logs above.\033[0m"
    exit 1
fi

# 6. Run Onboarding
echo -e "\033[0;32m✨ Build Successful! Launching Onboarding Wizard...\033[0m"
./target/release/openpaw onboard

echo -e "\n\033[0;32m✅ OpenPaw is ready! Run it anytime with: ./target/release/openpaw agent\033[0m"
