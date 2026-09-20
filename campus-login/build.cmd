@echo off
rem 编译脚本：把 target 目录指向纯 ASCII 路径，规避 MinGW 链接器对中文路径的兼容问题
setlocal
set CARGO_TARGET_DIR=%TEMP%\campus-login-build
echo [构建] CARGO_TARGET_DIR=%CARGO_TARGET_DIR%
cargo build --release
if errorlevel 1 (
    echo [失败] 编译未通过，请检查上面的错误输出
    exit /b 1
)
echo.
echo [成功] 产物位置：
echo   %CARGO_TARGET_DIR%\x86_64-pc-windows-gnu\release\campus-login.exe
endlocal
