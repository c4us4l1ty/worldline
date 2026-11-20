#!/usr/bin/env bash
# Exercise native startup at the minimum SDK on a disposable debug emulator.
set -euo pipefail

apk="${1:?usage: smoke-android.sh <debug-apk>}"
package=com.worldline.app.debug
activity="$package/com.worldline.app.MainActivity"

[[ "$(adb shell getprop ro.build.version.sdk | tr -d '\r')" == 26 ]]
adb install -r "$apk"
adb shell pm clear "$package"

startup="$(adb shell am start -W -n "$activity" | tr -d '\r')"
printf '%s\n' "$startup"
grep -q '^Status: ok$' <<< "$startup"

# File checks are debug-only. Assert the Rust library initialized its private
# state, not just that the launcher activity started. Never read file contents.
for ((attempt = 0; attempt < 30; attempt++)); do
    if adb shell run-as "$package" test -s worldline/worldline.sqlite &&
       adb shell run-as "$package" test -s worldline/.vault-key &&
       adb shell run-as "$package" test -s worldline/device_id &&
       adb shell pidof "$package" > /dev/null; then
        echo 'Android 8.0 native startup passed'
        exit 0
    fi
    sleep 1
done

echo 'Android 8.0 startup failed: app did not initialize its SQLite store and vault key' >&2
adb logcat -d -t 200 -s AndroidRuntime RustStdoutStderr >&2
exit 1
