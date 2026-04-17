@echo off
:: Auto-elevate to Administrator
net session >nul 2>&1
if %errorLevel% neq 0 (
    powershell -Command "Start-Process '%~f0' -Verb RunAs"
    exit /b
)

set SERVICE=OpenPaw
set BINARY=%USERPROFILE%\.cargo\bin\openpaw.exe
set WORKDIR=D:\pawworkspace
set LOG=%WORKDIR%\service.log
set NSSM=C:\nssm\nssm.exe

:MENU
cls
echo ==========================================
echo          OpenPaw Service Manager
echo ==========================================
echo.
for /f "tokens=3" %%s in ('sc query %SERVICE% 2^>nul ^| findstr STATE') do set STATE=%%s
if "%STATE%"=="RUNNING" (
    echo   Status: RUNNING
) else if "%STATE%"=="STOPPED" (
    echo   Status: STOPPED
) else (
    echo   Status: NOT INSTALLED
)
echo.
echo   [1] Install  ^& Start
echo   [2] Start
echo   [3] Stop
echo   [4] Restart
echo   [5] View Logs
echo   [6] Uninstall
echo   [7] Exit
echo.
set /p CHOICE=  Choose an option:

if "%CHOICE%"=="1" goto INSTALL
if "%CHOICE%"=="2" goto START
if "%CHOICE%"=="3" goto STOP
if "%CHOICE%"=="4" goto RESTART
if "%CHOICE%"=="5" goto LOGS
if "%CHOICE%"=="6" goto UNINSTALL
if "%CHOICE%"=="7" exit /b
goto MENU

:GET_NSSM
if exist "%NSSM%" goto :EOF
echo  Downloading nssm...
powershell -Command "$z='%TEMP%\nssm.zip'; Invoke-WebRequest 'https://nssm.cc/release/nssm-2.24.zip' -OutFile $z; Expand-Archive $z '%TEMP%\nssm_extracted' -Force; New-Item -ItemType Directory -Force -Path 'C:\nssm' | Out-Null; Copy-Item '%TEMP%\nssm_extracted\nssm-2.24\win64\nssm.exe' 'C:\nssm\nssm.exe' -Force"
if not exist "%NSSM%" (
    echo  ERROR: Failed to download nssm. Check your internet connection.
    pause
    goto MENU
)
echo  nssm ready.
goto :EOF

:INSTALL
echo.
call :GET_NSSM
if not exist "%NSSM%" goto MENU
if not exist "%BINARY%" (
    echo  ERROR: openpaw.exe not found at %BINARY%
    echo  Run: cargo install --path . inside d:\openpaw first.
    pause
    goto MENU
)
"%NSSM%" install %SERVICE% "%BINARY%"
"%NSSM%" set %SERVICE% AppParameters agent
"%NSSM%" set %SERVICE% AppDirectory "%WORKDIR%"
"%NSSM%" set %SERVICE% AppStdout "%LOG%"
"%NSSM%" set %SERVICE% AppStderr "%LOG%"
"%NSSM%" set %SERVICE% AppRotateFiles 1
"%NSSM%" set %SERVICE% AppRotateBytes 5242880
"%NSSM%" set %SERVICE% AppExit Default Restart
"%NSSM%" set %SERVICE% AppRestartDelay 5000
"%NSSM%" start %SERVICE%
echo.
echo  Done! OpenPaw is now running as a Windows service.
pause
goto MENU

:START
echo.
"%NSSM%" start %SERVICE%
pause
goto MENU

:STOP
echo.
echo  Stopping service...
"%NSSM%" stop %SERVICE%
:: Wait up to 10s for it to fully stop
set /a TRIES=0
:STOP_WAIT
sc query %SERVICE% | findstr "STOP_PENDING" >nul 2>&1
if errorlevel 1 goto STOP_DONE
set /a TRIES+=1
if %TRIES% geq 10 goto STOP_FORCE
timeout /t 1 /nobreak >nul
goto STOP_WAIT
:STOP_FORCE
echo  Taking too long — force killing...
taskkill /f /im openpaw.exe >nul 2>&1
:STOP_DONE
echo  Stopped.
pause
goto MENU

:RESTART
echo.
echo  Stopping service...
"%NSSM%" stop %SERVICE%
set /a TRIES=0
:RESTART_WAIT
sc query %SERVICE% | findstr "STOP_PENDING" >nul 2>&1
if errorlevel 1 goto RESTART_START
set /a TRIES+=1
if %TRIES% geq 10 (
    taskkill /f /im openpaw.exe >nul 2>&1
    goto RESTART_START
)
timeout /t 1 /nobreak >nul
goto RESTART_WAIT
:RESTART_START
"%NSSM%" start %SERVICE%
echo  Restarted.
pause
goto MENU

:LOGS
echo.
echo  Showing live logs from %LOG% (Ctrl+C to go back)
echo.
powershell -Command "Get-Content '%LOG%' -Wait -Tail 50"
goto MENU

:UNINSTALL
echo.
set /p CONFIRM=  Are you sure you want to uninstall? (y/n):
if /i "%CONFIRM%"=="y" (
    "%NSSM%" stop %SERVICE%
    timeout /t 3 /nobreak >nul
    taskkill /f /im openpaw.exe >nul 2>&1
    "%NSSM%" remove %SERVICE% confirm
    echo  Service removed.
)
pause
goto MENU
