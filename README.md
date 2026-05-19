# LanSend

LanSend is a Tauri v2 desktop application for local network file transfer. It uses a Vue 3 + TypeScript frontend and a Rust backend.

## Features

- Show local IPv6, IPv4, and device alias on the main screen
- Save LAN devices by IPv4 or IPv6 address
- Probe saved devices manually or on an automatic interval
- Drag files into the drop zone and send them to an online saved device
- Require the receiver to accept an incoming transfer before upload starts
- Configure alias, listening port, save folder, and refresh interval

## Requirements

### All platforms

Install these first:

- Node.js
- pnpm
- Rust toolchain with Cargo

### Linux / Ubuntu

Install the native libraries required by Tauri and WebKitGTK. On Ubuntu, use the package names recommended by the current Tauri v2 Linux setup guide for your distribution version.

You also need the `ip` command if you want LanSend to prefer stable IPv6 addresses on Linux. It is usually provided by the `iproute2` package.

## Install dependencies

From the project root:

```bash
pnpm install
```

## Start the app in development mode

From the project root:

```bash
pnpm tauri dev
```

This starts the Vite frontend and launches the Tauri desktop window.

If Tauri capabilities or native plugins were changed, fully restart the dev command instead of relying on hot reload.

## Build the frontend

To type-check and build the web frontend:

```bash
pnpm build
```

The frontend output is written to `dist/`.

## Check the Rust/Tauri backend

To check the Rust backend without creating an installer:

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

## Build desktop packages

To build release packages for the current operating system:

```bash
pnpm tauri build
```

The generated bundles are written under `src-tauri/target/release/bundle/`.

Build packages on each target platform when you need platform-native artifacts:

- Windows: build on Windows for Windows installers
- macOS: build on macOS for macOS app bundles or disk images
- Linux: build on Linux for Linux packages

## First run

1. Start two LanSend instances on devices in the same local network.
2. Confirm each device shows a reachable IPv4 or IPv6 address.
3. Use **Add Device** to save the other device address and port.
4. Click **Refresh** or wait for automatic refresh until the device appears online.
5. Drag one or more files into the drop zone.
6. Click the online saved device.
7. Confirm sending on the sender side.
8. Accept the incoming transfer on the receiver side.

Received files are saved to the configured save folder.

## Configuration

Open **Settings** in the app to configure:

- **Alias**: local display name. Chinese characters, English letters, and numbers are supported.
- **Default Port**: local listening port and default port for new saved devices.
- **Save Folder**: destination folder for received files.
- **Auto Refresh Interval**: how often saved devices are probed automatically, in seconds.

## Development commands

```bash
pnpm install          # install frontend dependencies
pnpm tauri dev       # start the desktop app in development mode
pnpm build           # type-check and build the frontend
pnpm tauri build     # build release desktop bundles
```
