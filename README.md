# Heartnet Apps

Small, fast Android apps written in Rust. Each app is a single APK that installs on a normal
phone, uses little memory and asks for as few permissions as it can.

| App | What it does |
| --- | --- |
| Gallery | Photos and videos, albums, a locked hidden album, trash, video gestures |
| Files | A file manager with search, storage overview and a 30 day trash |
| Notes | Plain text notes kept as files, with search, pinning and a 30 day trash |
| Home | A light home screen: app grid, dock, search |

Everything stays on the phone. See [PRIVACY.md](PRIVACY.md).

## Build

Needs Rust with the `aarch64-linux-android` target, `cargo-ndk`, the Android SDK (build-tools 34,
platform 34), NDK r27c and a JDK 17. Then, from an app folder (Windows PowerShell):

```
cargo test
powershell -File build-apk.ps1
```

APKs are signed with your own key, see [docs/SIGNING.md](docs/SIGNING.md). Install steps for
phones are in [docs/INSTALL.md](docs/INSTALL.md).

## Layout

- `apps/` one folder per app
- `crates/heartnet-kit/` code the apps share (exit handling, settings, theme, logging)
- `tools/build/` the shared APK build script
- `tools/icons/` the launcher icon generator

## Licence

Copyright (C) heartnetapps. Licensed under the GNU General Public License, version 3 or (at your
option) any later version. See [LICENSE](LICENSE). The names and logos are not covered by the licence.
