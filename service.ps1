# OpenPaw Windows Service Manager
# Run as Administrator: Right-click PowerShell → "Run as administrator"

param([string]$Action = "install")

$BinaryPath = "$env:USERPROFILE\.cargo\bin\openpaw.exe"
$WorkDir    = "D:\pawworkspace"
$ServiceName = "OpenPaw"

function Install-Service {
    # Check nssm
    if (-not (Get-Command nssm -ErrorAction SilentlyContinue)) {
        Write-Host "Installing nssm via winget..." -ForegroundColor Yellow
        winget install nssm --silent
        $env:PATH += ";$env:ProgramFiles\nssm"
    }

    if (-not (Test-Path $BinaryPath)) {
        Write-Error "openpaw.exe not found at $BinaryPath. Run: cargo install --path ."
        exit 1
    }

    Write-Host "Installing OpenPaw service..." -ForegroundColor Cyan
    nssm install $ServiceName $BinaryPath
    nssm set $ServiceName AppParameters    agent
    nssm set $ServiceName AppDirectory     $WorkDir
    nssm set $ServiceName AppStdout        "$WorkDir\service.log"
    nssm set $ServiceName AppStderr        "$WorkDir\service.log"
    nssm set $ServiceName AppRotateFiles   1
    nssm set $ServiceName AppRotateBytes   5242880
    nssm set $ServiceName AppExit Default  Restart
    nssm set $ServiceName AppRestartDelay  5000
    nssm start $ServiceName

    Write-Host ""
    Write-Host "Done! OpenPaw is running as a Windows service." -ForegroundColor Green
    Write-Host "Logs: $WorkDir\service.log"
}

function Remove-Service {
    nssm stop    $ServiceName
    nssm remove  $ServiceName confirm
    Write-Host "Service removed." -ForegroundColor Yellow
}

switch ($Action) {
    "install"   { Install-Service }
    "uninstall" { Remove-Service }
    "remove"    { Remove-Service }
    "start"     { nssm start   $ServiceName }
    "stop"      { nssm stop    $ServiceName }
    "restart"   { nssm restart $ServiceName }
    "status"    { nssm status  $ServiceName }
    "logs"      { Get-Content "$WorkDir\service.log" -Wait -Tail 50 }
    default     { Write-Host "Usage: .\service.ps1 [install|uninstall|start|stop|restart|status|logs]" }
}
