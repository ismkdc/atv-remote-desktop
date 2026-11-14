# ATV Remote Desktop

A desktop Apple TV remote for macOS, Windows and Linux built with Tauri 2, Rust, React and TypeScript. The interface follows the supplied remote reference, with a touch surface, playback controls, text input and keyboard navigation. All application text is in English.

## Development

Install Node.js, Rust stable and Python 3.15, plus the platform prerequisites listed at https://v2.tauri.app/start/prerequisites/. macOS requires Xcode Command Line Tools; Windows requires the C++ build tools and WebView2; Linux requires WebKitGTK 4.1 and GTK 3.

```sh
npm ci
python3 -m venv .venv
.venv/bin/python -m pip install --upgrade pip
.venv/bin/python -m pip install -r requirements.txt
npm run build:bridge
npm run tauri dev
```

On Windows, use `.venv\Scripts\python.exe` instead of `.venv/bin/python` in the setup commands.

Development uses the project's Python environment. Set `ATV_PYTHON` to override the interpreter. macOS discovery uses the system Bonjour daemon; Windows and Linux use pyatv mDNS discovery. The current machine and loopback addresses are excluded from the device list.

Release builds include a standalone Python helper, so users do not need to install Python or keep the source folder.

Validated dependency versions: Tauri Rust 2.12.5, Tauri API/CLI 2.12.4, React 19.3.1, TypeScript 7.0.2, Vite 8.3.8 and pyatv 0.18.0. `srptools` is pinned to 1.0.1 because pyatv 0.18.0 is incompatible with the keyword-only API introduced in srptools 2.0 (https://github.com/postlund/pyatv/issues/2934). The npm and Cargo lockfiles record the resolved dependencies. Run `npm update` and `cargo update --manifest-path src-tauri/Cargo.toml` to update within the declared constraints.

## Usage

Keep your Mac and Apple TV on the same local network. Open the device selector, select your Apple TV and enter the PIN displayed on the television. Swipe the touch surface to navigate or click to select.

- Arrow keys: navigate
- Enter: select
- Escape or Backspace: back
- Space: play or pause

Contextual controls refresh every second. Skip buttons appear only when the current media supports the corresponding operation and send an explicit ten-second interval. Text input appears only while a text field is focused on Apple TV and text replacement is available. Media information appears only when active playback metadata is available. The app does not expose an unsupported subtitles/audio menu. Volume and power support depend on the device, protocol and HDMI configuration. Unsupported operations display an error. Pairing credentials are stored in the application data directory with access restricted to the current user.

## Packaging

```sh
npm run package
```

This builds the standalone helper and creates the platform packages in `src-tauri/target/release/bundle`. Each platform and architecture is built on its matching runner. GitHub Actions builds macOS Apple Silicon/Intel (DMG), Windows x64 (NSIS and MSI), and Linux x64 (DEB and AppImage), checks the frozen helper IPC, and uploads installers as artifacts. Linux builds use Ubuntu 24.04 and target systems with compatible glibc and WebKitGTK.

For local macOS signing, set `APPLE_SIGNING_IDENTITY` to an installed Apple-issued code-signing certificate before `npm run package`; the same identity signs both the helper and the app. CI currently produces unsigned packages. Developer ID distribution signing and notarization require CI certificate secrets and are not configured.

## Validation

The frontend production build, Rust checks and standalone helper discovery have been tested locally. The packaged app opens successfully. The packaged macOS app discovers Living Room through Bonjour. macOS Local Network permission is required for device connections. PIN pairing and physical Apple TV commands have not yet been verified. Windows/Linux installers must be validated on their CI runners and with physical devices on those platforms.

## References

- https://github.com/bsharper/atv-desktop-remote
- https://github.com/ismkdc/openwrt-manager
- https://pyatv.dev
