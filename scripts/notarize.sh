#!/bin/bash
# Notarize a signed .app with Apple and staple the ticket to it, so Gatekeeper
# opens it without "Apple could not verify…". Credentials: an App Store
# Connect API key in NOTARY_KEY_ID, NOTARY_ISSUER_ID and NOTARY_KEY_P8 (the
# key's contents), or a notarytool keychain profile named in NOTARY_PROFILE.
# Without either it says so and exits 0, so a local or ad-hoc build goes on.
set -euo pipefail
APP="${1:?usage: notarize.sh \"path/to/App.app\"}"
# (codesign's output goes through a variable: under pipefail, grep -q closing
# the pipe early would make a signed app look unsigned.)
SIGNATURE="$(codesign -dvv "$APP" 2>&1 || true)"
if ! printf '%s' "$SIGNATURE" | grep -q "Authority=Developer ID Application"; then
  echo "notarize: $APP is not signed with a Developer ID; nothing to notarize"
  exit 0
fi
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
AUTH=()
if [ -n "${NOTARY_PROFILE:-}" ]; then
  AUTH=(--keychain-profile "$NOTARY_PROFILE")
elif [ -n "${NOTARY_KEY_ID:-}" ] && [ -n "${NOTARY_ISSUER_ID:-}" ] && [ -n "${NOTARY_KEY_P8:-}" ]; then
  printf '%s\n' "$NOTARY_KEY_P8" > "$TMP/AuthKey.p8"
  AUTH=(--key "$TMP/AuthKey.p8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
else
  echo "notarize: no NOTARY_PROFILE nor NOTARY_KEY_ID/NOTARY_ISSUER_ID/NOTARY_KEY_P8; skipping"
  exit 0
fi
ditto -c -k --keepParent "$APP" "$TMP/upload.zip"
echo "notarize: submitting $(basename "$APP")…"
xcrun notarytool submit "$TMP/upload.zip" "${AUTH[@]}" --wait --timeout 30m --output-format plist > "$TMP/result.plist"
STATUS="$(/usr/libexec/PlistBuddy -c 'Print :status' "$TMP/result.plist")"
ID="$(/usr/libexec/PlistBuddy -c 'Print :id' "$TMP/result.plist")"
if [ "$STATUS" != "Accepted" ]; then
  echo "notarize: $STATUS (submission $ID); Apple's log:"
  xcrun notarytool log "$ID" "${AUTH[@]}" || true
  exit 1
fi
xcrun stapler staple "$APP"
spctl --assess --type execute -v "$APP"
echo "notarize: accepted and stapled"
