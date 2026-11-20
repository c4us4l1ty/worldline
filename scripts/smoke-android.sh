#!/usr/bin/env bash
# Exercise native startup at the minimum SDK on a disposable debug emulator.
set -euo pipefail

apk="${1:?usage: smoke-android.sh <debug-apk>}"
package=com.worldline.app.debug
activity="$package/com.worldline.app.MainActivity"

fail() {
    local detail="$1"
    if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
        detail="${detail//%/%25}"
        detail="${detail//$'\r'/%0D}"
        detail="${detail//$'\n'/%0A}"
        printf '::error title=Android 8 smoke test::%s\n' "$detail"
    else
        printf '%s\n' "$detail" >&2
    fi
    exit 1
}

[[ "$(adb shell getprop ro.build.version.sdk | tr -d '\r')" == 26 ]] || fail 'Expected an API 26 emulator'
install="$(adb install -r "$apk" 2>&1)" || fail "Could not install APK: $install"
printf '%s\n' "$install"
clear="$(adb shell pm clear "$package" 2>&1)" || fail "Could not reset app data: $clear"

startup="$(adb shell am start -W -n "$activity" 2>&1 | tr -d '\r')" || fail "Could not launch app: $startup"
printf '%s\n' "$startup"
grep -q '^Status: ok$' <<< "$startup" || fail "Activity did not report successful launch: $startup"

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

state="$(adb shell run-as "$package" ls -a worldline 2>&1 || true)"
logs="$(adb logcat -d -s AndroidRuntime RustStdoutStderr | tail -c 2200 || true)"
fail "Native state did not initialize. App directory: $state
$logs"
