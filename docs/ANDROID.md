# Worldline for Android

Worldline's Android app is the Tauri host for the same Dioxus UI and Rust
engine used on desktop. The APK embeds the UI; it does not need a computer,
a web server, an account, or an internet connection to run manual goals.
The Android application ID is `com.worldline.app` (`com.worldline.app.debug`
for test builds). Minimum Android: 8.0 (API 26). The UI runs in Android WebView, so install
the latest Android System WebView or Chrome version offered for your device.
The stock WebView shipped with Android 8.0 is not a supported UI runtime.

## Get the test APK

After the branch is pushed, the **Android APK** GitHub Actions workflow builds
and uploads **Worldline-Android-debug** for ARM64 and 32-bit ARM phones, plus x86-64 emulators.
Download and unzip the artifact to get `Worldline-debug.apk`. Sideload it, or
install it with USB debugging enabled:

```sh
adb install -r Worldline-debug.apk
```

This is a **debug-signed test build**, not a Play Store or production release.
Each fresh Actions runner can sign with a different debug key, so an APK from
another run may not update in place. Uninstalling erases the app's local data;
back up the recovery words and sync anything you want to retain before doing
so. Builds signed with a consistent key can be updated using `adb install -r`.

## Build locally

Use Linux or macOS with Bash; desktop GTK/WebKit packages are not required.
Install JDK 17 and Android Studio or the Android SDK command-line tools.
Set `JAVA_HOME` and `ANDROID_HOME`, and put `sdkmanager` and `adb` on `PATH`.
From a shell with Rust stable and `rustup`:

```sh
sdkmanager --licenses
sdkmanager 'platform-tools' 'platforms;android-36' 'build-tools;36.0.0' 'ndk;29.0.13846066'
export NDK_HOME="$ANDROID_HOME/ndk/29.0.13846066"
rustup target add wasm32-unknown-unknown aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install tauri-cli --version 2.11.4 --locked
cargo install dioxus-cli --version 0.7.10 --locked

cd crates/wl-app
cargo tauri android build --apk --debug --target aarch64 armv7 x86_64
adb install -r gen/android/app/build/outputs/apk/universal/debug/*.apk
```

For an ARM64-only phone build, use `--target aarch64`; for a 32-bit ARM
phone, use `--target armv7`; for an x86-64 emulator, use `--target x86_64`. `gen/android` is checked in: do not replace it
with `android init` output. The `beforeBuildCommand` packages the UI first,
then the Tauri CLI cross-compiles the Rust library and assembles the APK.
The browser harness (`dx serve`) is not the Android app; it uses mock data.

## First run and device checks

CI verifies the packaged minimum SDK and installs the APK on an Android 8.0
emulator. The smoke check confirms native startup initializes the app's
private SQLite and vault files. It does not validate the UI on the stock
emulator WebView. Run the following checks on an Android 8 device with an
updated WebView, and repeat on a current Android version.

1. Launch in airplane mode. Tap **Menu → New goal**, enter an objective,
   and tap **Create manually**. The canvas should show the next directive.
2. Open **Menu → Entropy Log** and mark a task done. Force-stop and reopen
   the app, still offline, and check that the ledger and canvas are current.
3. Open Settings, then an AI picker. System Back should dismiss the picker
   first and go back to the canvas on the next press. Test a short screen
   with the keyboard open: the objective and its actions should stay reachable
   by scrolling. Test gesture and three-button navigation if available.
4. Try **Menu → Evening check-in** without a keyboard shortcut. As on
   desktop, recording a check-in requires an unlocked identity.
5. For sync, generate or restore a test identity in Settings. Write down the
   twelve words and finish verification before leaving the onboarding screen.
   Use the same phrase and a **phone-reachable HTTPS relay URL** on both
   devices, then sync. `localhost` from the phone is the phone, not the
   desktop. AI planning additionally needs a configured model and API key.

Android uses the app's private storage for SQLite, the Stronghold snapshot,
and its local key file. That directory is excluded from OS backup and device
transfer. Android screenshots, screen recordings, and Recents thumbnails are
blocked because identity and key screens can contain secrets. This is not
Android Keystore integration; the existing Stronghold design is unchanged.
Without a configured relay and a recorded phrase, uninstalling or clearing app
data can permanently lose local work.

Release signing and Play Store distribution are not configured. Do not use a
shared debug APK to store real recovery phrases or production API keys.
