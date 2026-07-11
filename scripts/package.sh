#!/bin/bash
# MetroForge release packaging script
# Usage: package.sh <os> <version>
# os: linux, windows, or macos
# version: release version (e.g., 0.1.0-alpha)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

OS="${1:-}"
VERSION="${2:-}"

# Validate arguments
if [[ -z "$OS" ]] || [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <os> <version>"
    echo "  os: linux, windows, or macos"
    echo "  version: release version (e.g., 0.1.0-alpha)"
    exit 1
fi

case "$OS" in
    linux|windows|macos)
        ;;
    *)
        echo "Error: Invalid OS '$OS'. Must be one of: linux, windows, macos"
        exit 1
        ;;
esac

# Create release artifacts directory
RELEASE_DIR="$PROJECT_ROOT/release-artifacts"
mkdir -p "$RELEASE_DIR"

# Staging directory
STAGE_DIR="$PROJECT_ROOT/target/packaging-stage-$OS"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"

# Helper functions
check_file() {
    local file="$1"
    local description="$2"
    if [[ ! -f "$file" ]]; then
        echo "Error: Missing $description"
        echo "  Expected at: $file"
        case "$description" in
            "client binary")
                echo "  To build: cd $PROJECT_ROOT && cargo build --release -p mf-game"
                ;;
            "sidecar binary"*)
                echo "  To build: Run the sidecar build step (bun build --compile)"
                ;;
        esac
        exit 1
    fi
}

make_executable() {
    local file="$1"
    chmod +x "$file"
}

write_readme() {
    local readme_path="$1"
    local os="$2"
    local version="$3"

    cat > "$readme_path" << 'EOF'
# MetroForge Release

## How to Run

### Linux
1. Extract the archive:
   tar xzf metroforge-<version>-linux-x64.tar.gz -C /path/to/install
2. Navigate to the extracted directory
3. Run the game:
   ./metroforge

Note: Both metroforge and metroforge-sidecar binaries are executable. They work
together automatically without additional setup.

### macOS
1. Extract the archive
2. Releases are ad-hoc signed (not Developer ID / notarized). Right-click
   metroforge and select "Open" (plain double-click is blocked on first launch).
3. If you see a Gatekeeper warning about unverified developer:
   - Click "Cancel" (do not click "Move to Trash")
   - Open System Settings > Privacy & Security
   - Scroll to "Security" section
   - You should see metroforge listed with "was blocked"
   - Click "Open Anyway"
   - Confirm in the dialog that follows
4. The game will now launch

Note: Both metroforge and metroforge-sidecar binaries work together automatically.

### Windows
1. Extract the archive to your desired location
2. Double-click metroforge.exe to launch the game

Note: Releases are not Authenticode-signed, so Windows Defender SmartScreen
will usually warn on first run:
- Click "More info" in the SmartScreen dialog
- Click "Run anyway" to proceed
- The game will launch normally
- A second launch focuses the existing window (no second sidecar)

## System Requirements

MetroForge automatically detects your GPU and adjusts graphics quality accordingly.
Modern integrated GPUs (Intel UHD, Apple Silicon) are supported.

## Potato Mode

If the game runs slowly or has graphics glitches, it can automatically fall back
to lower-quality rendering. You can also manually configure this by creating a
config file (see game documentation for details).

## Additional Notes

- Both the main executable (metroforge) and sidecar (metroforge-sidecar) are
  included in this archive and work together automatically.
- On Linux/macOS, the executables are marked executable. Windows executables run
  normally.
- Permissions: Linux/macOS users should not need to manually chmod these files,
  but if you encounter permission errors, ensure the binaries have execute
  permissions.
EOF

    # Remove em-dashes from the readme (per player-facing copy guidelines).
    # Portable across GNU and BSD sed: BSD (macOS) sed -i requires a suffix
    # argument, which broke the v0.1.0-alpha macos release job.
    sed 's/[–—]/-/g' "$readme_path" > "$readme_path.tmp" && mv "$readme_path.tmp" "$readme_path"
}

# Package based on OS
case "$OS" in
    linux)
        echo "Packaging MetroForge $VERSION for Linux..."

        # Check for required binaries
        CLIENT_BIN="$PROJECT_ROOT/target/release/metroforge"
        SIDECAR_BIN="$PROJECT_ROOT/dist-sidecar/metroforge-sidecar"

        check_file "$CLIENT_BIN" "client binary"
        check_file "$SIDECAR_BIN" "sidecar binary (Linux)"

        # Stage files
        cp "$CLIENT_BIN" "$STAGE_DIR/metroforge"
        cp "$SIDECAR_BIN" "$STAGE_DIR/metroforge-sidecar"
        cp "$PROJECT_ROOT/crates/mf-game/assets/fonts/OFL.txt" "$STAGE_DIR/OFL.txt"
        cp "$PROJECT_ROOT/packaging/icon.png" "$STAGE_DIR/metroforge.png"
        cp "$PROJECT_ROOT/packaging/linux/metroforge.desktop" "$STAGE_DIR/metroforge.desktop"

        # Make binaries executable
        make_executable "$STAGE_DIR/metroforge"
        make_executable "$STAGE_DIR/metroforge-sidecar"

        # Create README
        write_readme "$STAGE_DIR/README-dist.txt" "linux" "$VERSION"

        # Package as tar.gz
        ARTIFACT_NAME="metroforge-${VERSION}-linux-x64.tar.gz"
        ARTIFACT_PATH="$RELEASE_DIR/$ARTIFACT_NAME"

        cd "$STAGE_DIR"
        tar -czf "$ARTIFACT_PATH" \
            metroforge \
            metroforge-sidecar \
            OFL.txt \
            README-dist.txt \
            metroforge.png \
            metroforge.desktop

        echo "Successfully created: $ARTIFACT_PATH"
        echo "Archive contents:"
        tar -tzf "$ARTIFACT_PATH"
        ;;

    windows)
        echo "Packaging MetroForge $VERSION for Windows..."

        # Check for required binaries
        CLIENT_BIN="${MF_WIN_CLIENT:-$PROJECT_ROOT/target/x86_64-pc-windows-msvc/release/metroforge.exe}"
        SIDECAR_BIN="$PROJECT_ROOT/dist-sidecar/metroforge-sidecar.exe"

        check_file "$CLIENT_BIN" "client binary"
        check_file "$SIDECAR_BIN" "sidecar binary (Windows)"

        # Stage files
        cp "$CLIENT_BIN" "$STAGE_DIR/metroforge.exe"
        cp "$SIDECAR_BIN" "$STAGE_DIR/metroforge-sidecar.exe"
        cp "$PROJECT_ROOT/crates/mf-game/assets/fonts/OFL.txt" "$STAGE_DIR/OFL.txt"

        cp "$PROJECT_ROOT/packaging/icon.ico" "$STAGE_DIR/icon.ico"

        # Create README with Windows-specific notes
        write_readme "$STAGE_DIR/README-dist.txt" "windows" "$VERSION"

        # Package as zip
        ARTIFACT_NAME="metroforge-${VERSION}-windows-x64.zip"
        ARTIFACT_PATH="$RELEASE_DIR/$ARTIFACT_NAME"

        cd "$STAGE_DIR"
        if command -v zip >/dev/null 2>&1; then
            zip -q -r "$ARTIFACT_PATH" \
                metroforge.exe \
                metroforge-sidecar.exe \
                OFL.txt \
                README-dist.txt
        else
            # Fallback to PowerShell on Windows if zip is not available
            powershell -Command "Compress-Archive -Path metroforge.exe, metroforge-sidecar.exe, OFL.txt, README-dist.txt -DestinationPath '$ARTIFACT_PATH'"
        fi

        echo "Successfully created: $ARTIFACT_PATH"

        # NSIS installer (setup exe with Start Menu/desktop shortcuts and
        # uninstaller). makensis runs fine on the Linux release runner.
        if command -v makensis >/dev/null 2>&1; then
            INSTALLER_PATH="$RELEASE_DIR/metroforge-${VERSION}-windows-x64-setup.exe"
            makensis -V2 \
                -DVERSION="$VERSION" \
                -DSTAGEDIR="$STAGE_DIR" \
                -DOUTFILE="$INSTALLER_PATH" \
                "$PROJECT_ROOT/packaging/windows/installer.nsi"
            echo "Successfully created: $INSTALLER_PATH"
        else
            echo "makensis not found; skipping Windows installer (zip only)"
        fi
        ;;

    macos)
        echo "Packaging MetroForge $VERSION for macOS..."

        # Check for required binaries
        CLIENT_BIN="${MF_MAC_CLIENT:-$PROJECT_ROOT/target/release/metroforge}"
        SIDECAR_BIN_SOURCE="$PROJECT_ROOT/dist-sidecar/metroforge-sidecar-darwin-arm64"

        check_file "$CLIENT_BIN" "client binary"
        check_file "$SIDECAR_BIN_SOURCE" "sidecar binary (macOS ARM64)"

        # Stage files
        cp "$CLIENT_BIN" "$STAGE_DIR/metroforge"
        cp "$SIDECAR_BIN_SOURCE" "$STAGE_DIR/metroforge-sidecar"
        cp "$PROJECT_ROOT/crates/mf-game/assets/fonts/OFL.txt" "$STAGE_DIR/OFL.txt"

        # Make binaries executable
        make_executable "$STAGE_DIR/metroforge"
        make_executable "$STAGE_DIR/metroforge-sidecar"

        # Create README with macOS-specific notes
        write_readme "$STAGE_DIR/README-dist.txt" "macos" "$VERSION"

        # Package as zip (macOS convention)
        ARTIFACT_NAME="metroforge-${VERSION}-macos-arm64.zip"
        ARTIFACT_PATH="$RELEASE_DIR/$ARTIFACT_NAME"

        cd "$STAGE_DIR"
        if command -v zip >/dev/null 2>&1; then
            zip -q -r "$ARTIFACT_PATH" \
                metroforge \
                metroforge-sidecar \
                OFL.txt \
                README-dist.txt
        else
            # Fallback to ditto on macOS
            ditto -c -k --sequesterRsrc . "$ARTIFACT_PATH"
        fi

        echo "Successfully created: $ARTIFACT_PATH"

        # .app bundle + .dmg installer. Client and sidecar stay siblings in
        # Contents/MacOS (sidecar.rs resolves next to the running exe).
        # hdiutil exists only on macOS; skip gracefully elsewhere.
        if command -v hdiutil >/dev/null 2>&1; then
            APP_DIR="$STAGE_DIR/dmg-root/MetroForge.app"
            mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
            cp "$STAGE_DIR/metroforge" "$APP_DIR/Contents/MacOS/metroforge"
            cp "$STAGE_DIR/metroforge-sidecar" "$APP_DIR/Contents/MacOS/metroforge-sidecar"
            cp "$STAGE_DIR/OFL.txt" "$APP_DIR/Contents/Resources/OFL.txt"
            cp "$PROJECT_ROOT/packaging/icon.icns" "$APP_DIR/Contents/Resources/icon.icns"
            sed "s/__VERSION__/$VERSION/g" "$PROJECT_ROOT/packaging/macos/Info.plist" \
                > "$APP_DIR/Contents/Info.plist"
            make_executable "$APP_DIR/Contents/MacOS/metroforge"
            make_executable "$APP_DIR/Contents/MacOS/metroforge-sidecar"
            # Ad-hoc sign so Gatekeeper shows the standard right-click-Open
            # flow instead of refusing outright on arm64 (unsigned binaries
            # are killed on Apple Silicon).
            codesign --force --deep -s - "$APP_DIR" || echo "codesign failed; continuing unsigned"
            ln -sf /Applications "$STAGE_DIR/dmg-root/Applications"
            DMG_PATH="$RELEASE_DIR/metroforge-${VERSION}-macos-arm64.dmg"
            hdiutil create -volname "MetroForge $VERSION" -srcfolder "$STAGE_DIR/dmg-root" \
                -ov -format UDZO "$DMG_PATH"
            echo "Successfully created: $DMG_PATH"
        else
            echo "hdiutil not found; skipping macOS dmg (zip only)"
        fi
        ;;
esac

# Cleanup staging directory
rm -rf "$STAGE_DIR"

echo "Release packaging complete!"
echo "Output: $ARTIFACT_PATH"
