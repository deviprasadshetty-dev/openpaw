# OpenPaw One-Step Installer for Windows
# Usage: powershell -ExecutionPolicy ByPass -Command "irm https://raw.githubusercontent.com/deviprasadshetty-dev/openpaw/main/install.ps1 | iex"

$ErrorActionPreference = "Stop"

Write-Host "🐾 Starting OpenPaw Installation..." -ForegroundColor Cyan

# 1. Check for Git
if (!(Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Host "❌ Git not found. Please install Git from https://git-scm.com/" -ForegroundColor Red
    exit 1
}

# 2. Check for Rust
if (!(Get-Command rustup -ErrorAction SilentlyContinue)) {
    Write-Host "🦀 Rust not found. Installing Rustup..." -ForegroundColor Yellow
    Invoke-WebRequest -Uri "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" -OutFile "rustup-init.exe"
    Start-Process -FilePath ".\rustup-init.exe" -ArgumentList "-y" -Wait
    Remove-Item "rustup-init.exe"
    # Update current session path
    $env:Path += ";$env:USERPROFILE\.cargo\bin"
}

# 3. Clone if not in repo
if (!(Test-Path "Cargo.toml")) {
    Write-Host "📂 Cloning OpenPaw from GitHub..." -ForegroundColor Cyan
    git clone https://github.com/deviprasadshetty-dev/openpaw.git
    Set-Location openpaw
}

# 4. Build OpenPaw
Write-Host "🏗️  Building OpenPaw (this may take a few minutes)..." -ForegroundColor Cyan
cargo build --release

# 5. Check if build succeeded
if (!(Test-Path ".\target\release\openpaw.exe")) {
    Write-Host "❌ Build failed. Please check the logs above." -ForegroundColor Red
    exit 1
}

# 6. Run Onboarding
Write-Host "✨ Build Successful! Launching Onboarding Wizard..." -ForegroundColor Green
.\target\release\openpaw onboard

Write-Host "`n✅ OpenPaw is ready! Run it anytime with: .\target\release\openpaw agent" -ForegroundColor Green
