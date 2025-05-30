@echo off
setlocal enabledelayedexpansion

if "%1"=="" goto help
if "%1"=="help" goto help
if "%1"=="setup" goto setup
if "%1"=="dev" goto dev
if "%1"=="test" goto test
if "%1"=="fmt" goto fmt
if "%1"=="lint" goto lint
if "%1"=="build" goto build
if "%1"=="release" goto release
if "%1"=="clean" goto clean
if "%1"=="docs" goto docs
if "%1"=="docs-build" goto docs-build
if "%1"=="docker" goto docker
if "%1"=="compose" goto compose
if "%1"=="package" goto package

echo δ֪����: %1
goto help

:help
echo bili-sync ��������:
echo.
echo ��������:
echo   setup     - ���ÿ�������
echo   dev       - ��������������
echo   test      - ���в���
echo   fmt       - ��ʽ������
echo   lint      - ������
echo.
echo ��������:
echo   build     - ������Ŀ
echo   release   - ���������汾
echo   clean     - �������ļ�
echo   package   - ���Դ����
echo.
echo �ĵ�����:
echo   docs      - �����ĵ�������
echo   docs-build- �����ĵ�
echo.
echo Docker ����:
echo   docker    - ���� Docker ����
echo   compose   - ���� Docker Compose
echo.
echo �÷�: make.bat ^<����^>
goto end

:setup
echo �������ÿ�������...
echo ��� Rust ����...
cargo --version >nul 2>&1
if errorlevel 1 (
    echo δ�ҵ� Rust���밲װ Rust: https://rustup.rs/
    exit /b 1
)
echo Rust ��������

echo ��� Node.js ����...
node --version >nul 2>&1
if errorlevel 1 (
    echo δ�ҵ� Node.js���밲װ Node.js: https://nodejs.org/
    exit /b 1
)
echo Node.js ��������

echo ��װǰ������...
cd web
npm install
if errorlevel 1 (
    echo ��װǰ������ʧ��
    exit /b 1
)
echo ǰ��������װ���

echo ����ǰ��...
npm run build
if errorlevel 1 (
    echo ǰ�˹���ʧ��
    exit /b 1
)
cd ..
echo ǰ�˹������

echo ��װ Rust ����...
cargo check
if errorlevel 1 (
    echo ��װ Rust ����ʧ��
    exit /b 1
)
echo Rust ������װ���

echo ��װ�ĵ�����...
cd docs
npm install
if errorlevel 1 (
    echo ��װ�ĵ�����ʧ��
    exit /b 1
)
cd ..
echo �ĵ�������װ���

echo ���������������!
goto end

:dev
echo ������������������...
echo ���� Rust ���...
start "Rust Backend" cmd /k "cargo run --bin bili-sync-rs"
timeout /t 2 /nobreak >nul
echo ���� Svelte ǰ��...
start "Svelte Frontend" cmd /k "cd web && npm run dev"
echo ���з���������!
echo ��� API: http://localhost:12345
echo ǰ�� UI: http://localhost:5173
goto end

:test
echo �������в���...
cargo test
if errorlevel 1 (
    echo ����ʧ��
    exit /b 1
) else (
    echo ���в���ͨ��
)
goto end

:fmt
echo ���ڸ�ʽ������...
cargo fmt
echo �����ʽ�����
goto end

:lint
echo ���ڼ�����...
cargo clippy -- -D warnings
goto end

:build
echo ���ڹ�����Ŀ...
echo [DEBUG] ��ʼǰ�˹���...
cd web
if not exist "node_modules" (
    echo ��װǰ������...
    call npm install
    if errorlevel 1 (
        echo ��װǰ������ʧ��
        exit /b 1
    )
)
echo [DEBUG] ִ�� npm run build...
call npm run build
if errorlevel 1 (
    echo ǰ�˹���ʧ��
    exit /b 1
)
echo [DEBUG] ǰ�˹�����ɣ����ظ�Ŀ¼...
cd ..
echo [DEBUG] ��ʼ Rust ��˹���...
cargo build
if errorlevel 1 (
    echo ��˹���ʧ��
    exit /b 1
)
echo [DEBUG] ��˹������
echo ��Ŀ�������
goto end

:release
echo ���ڹ��������汾...
cd web
if not exist "node_modules" (
    echo ��װǰ������...
    npm install
    if errorlevel 1 (
        echo ��װǰ������ʧ��
        exit /b 1
    )
)
npm run build
if errorlevel 1 (
    echo ǰ�˹���ʧ��
    exit /b 1
)
cd ..
cargo build --release
if errorlevel 1 (
    echo ��˹���ʧ��
    exit /b 1
)
echo �����汾�������
goto end

:clean
echo �����������ļ�...
cargo clean
if exist "web\build" rmdir /s /q "web\build"
if exist "web\.svelte-kit" rmdir /s /q "web\.svelte-kit"
if exist "web\node_modules" rmdir /s /q "web\node_modules"
if exist "docs\.vitepress\dist" rmdir /s /q "docs\.vitepress\dist"
if exist "docs\node_modules" rmdir /s /q "docs\node_modules"
echo �������
goto end

:package
echo ���ڴ��Դ����...
echo ���� 1: �������ļ�...
call :clean

echo ���� 2: ����Դ�����...
:: ʹ�� PowerShell ��ȡ��ǰ����ʱ�䣨��ʽ��YYYY-MM-DD_HH-MM-SS��
for /f %%i in ('powershell -Command "Get-Date -Format 'yyyy-MM-dd_HH-mm-ss'"') do set timestamp=%%i
set packageName=bili-sync-source-%timestamp%.zip

echo ������: %packageName%

:: ������ʱĿ¼
set tempDir=temp_package
if exist "%tempDir%" rmdir /s /q "%tempDir%"
mkdir "%tempDir%"

:: �����ļ�
echo ����: .github
if exist ".github" (
    xcopy /s /e /q ".github" "%tempDir%\.github\" >nul 2>&1
    if errorlevel 1 echo ����: ���� .github ʧ��
) else (
    echo ����: δ�ҵ� .github �ļ���
)

echo ����: crates
if exist "crates" (
    xcopy /s /e /q "crates" "%tempDir%\crates\" >nul 2>&1
    if errorlevel 1 echo ����: ���� crates ʧ��
) else (
    echo ����: δ�ҵ� crates �ļ���
)

echo ����: web
if exist "web" (
    xcopy /s /e /q "web" "%tempDir%\web\" >nul 2>&1
    if errorlevel 1 echo ����: ���� web ʧ��
) else (
    echo ����: δ�ҵ� web �ļ���
)

echo ����: docs
if exist "docs" (
    xcopy /s /e /q "docs" "%tempDir%\docs\" >nul 2>&1
    if errorlevel 1 echo ����: ���� docs ʧ��
) else (
    echo ����: δ�ҵ� docs �ļ���
)

echo ����: scripts
if exist "scripts" (
    xcopy /s /e /q "scripts" "%tempDir%\scripts\" >nul 2>&1
    if errorlevel 1 echo ����: ���� scripts ʧ��
) else (
    echo ����: δ�ҵ� scripts �ļ���
)

echo ����: assets
if exist "assets" (
    xcopy /s /e /q "assets" "%tempDir%\assets\" >nul 2>&1
    if errorlevel 1 echo ����: ���� assets ʧ��
) else (
    echo ����: δ�ҵ� assets �ļ���
)

echo ����: Cargo.toml
copy "Cargo.toml" "%tempDir%\" >nul
echo ����: Cargo.lock
copy "Cargo.lock" "%tempDir%\" >nul
echo ����: Dockerfile
copy "Dockerfile" "%tempDir%\" >nul
echo ����: docker-compose.yml
copy "docker-compose.yml" "%tempDir%\" >nul
echo ����: README.md
copy "README.md" "%tempDir%\" >nul
echo ����: rustfmt.toml
copy "rustfmt.toml" "%tempDir%\" >nul
echo ����: .gitignore
copy ".gitignore" "%tempDir%\" >nul
echo ����: .dockerignore
copy ".dockerignore" "%tempDir%\" >nul
echo ����: config.toml
copy "config.toml" "%tempDir%\" >nul
echo ����: make.bat
copy "make.bat" "%tempDir%\" >nul

:: ������ʱĿ¼�еĲ���Ҫ��
if exist "%tempDir%\web\node_modules" rmdir /s /q "%tempDir%\web\node_modules"
if exist "%tempDir%\web\build" rmdir /s /q "%tempDir%\web\build"
if exist "%tempDir%\web\.svelte-kit" rmdir /s /q "%tempDir%\web\.svelte-kit"
if exist "%tempDir%\docs\node_modules" rmdir /s /q "%tempDir%\docs\node_modules"
if exist "%tempDir%\docs\.vitepress\dist" rmdir /s /q "%tempDir%\docs\.vitepress\dist"

:: ʹ�� PowerShell ���� ZIP
echo ���ڴ��� ZIP ��...
powershell -Command "Compress-Archive -Path '%tempDir%\*' -DestinationPath '%packageName%' -Force"

if exist "%packageName%" (
    echo �������ɹ�!
    for %%A in ("%packageName%") do (
        set /a sizeInMB=%%~zA/1024/1024
        echo �ļ�: %%~nxA
        echo ��С: !sizeInMB! MB
    )
) else (
    echo ������ʧ��
    exit /b 1
)

:: ����
rmdir /s /q "%tempDir%"
echo ������!
goto end

:docs
echo ���������ĵ�������...
cd docs
if not exist "node_modules" (
    echo ��װ�ĵ�����...
    call npm install
    if errorlevel 1 (
        echo ��װ�ĵ�����ʧ��
        exit /b 1
    )
)
npm run docs:dev
cd ..
goto end

:docs-build
echo ���ڹ����ĵ�...
cd docs
if not exist "node_modules" (
    echo ��װ�ĵ�����...
    call npm install
    if errorlevel 1 (
        echo ��װ�ĵ�����ʧ��
        exit /b 1
    )
)
npm run docs:build
cd ..
echo �ĵ��������
goto end

:docker
echo ���ڹ��� Docker ����...
docker build -t bili-sync .
goto end

:compose
echo �������� Docker Compose...
docker-compose up -d
goto end

:end