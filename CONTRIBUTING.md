# Contributing

Build on macOS or Linux with `./scripts/build.sh`, or on Windows with `.\scripts\build.ps1` (Rust stable; see the README for the Linux packages). Run `cargo test` and the built executable with `--self-test`.

Use `--demo` for screenshots so no private project names or terminal titles appear. Verify the native window, search, state filter, selection, menu-bar reopening, reduced motion and reconnection where relevant.

Keep monitoring read-only and truthful to Herdr states. Never insert synthetic agents into live mode.

Preserve upstream MIT notices and o_lobster attribution. New assets need provenance and license records in Resources/Licenses. Update the asset manifest when adding/modifying PNGs. Do not label CC BY artwork as MIT.

Bug reports should contain anonymized steps, the OS and Herdr versions. Do not post credentials, live snapshots or private terminal content.

## Commits and releases

Commit subjects follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), since they decide the next version:

- `fix: …` makes a patch release (2.1.2 → 2.1.3) and goes under **Fixed**.
- `feat: …` makes a minor release (2.1.2 → 2.2.0) and goes under **Added**; `perf: …` goes under **Changed**.
- `feat!: …`, or a `BREAKING CHANGE:` footer, makes a major release (2.1.2 → 3.0.0).
- `docs:`, `test:`, `refactor:`, `chore:` and `ci:` change no version and stay out of the changelog.

Releases are cut by [release-please](https://github.com/googleapis/release-please) (`.github/workflows/release-please.yml`). Every push to `main` keeps a `release: X.Y.Z` pull request up to date with the version in `Cargo.toml` and `Cargo.lock` and a new `CHANGELOG.md` section built from the commits. Edit that section in the pull request before merging if the commit subjects need prose. Merging it tags `vX.Y.Z`, publishes the GitHub release and attaches the macOS, Linux and Windows builds. Do not bump versions or tag by hand; the app bundle takes its version from `Cargo.toml` at build time.

The macOS build is signed and notarized when the repository has these secrets (`.github/workflows/release.yml`); without them it is signed ad-hoc and Gatekeeper blocks it on download:

| Secret | What |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | A "Developer ID Application" certificate with its private key, exported from Keychain Access as `.p12` and encoded with `base64 -i cert.p12`. |
| `MACOS_CERTIFICATE_PASSWORD` | The password given when exporting it. |
| `NOTARY_KEY_ID`, `NOTARY_ISSUER_ID`, `NOTARY_KEY_P8` | An App Store Connect API key (Users and Access > Integrations > Team Keys, role Developer): its Key ID, the Issuer ID, and the contents of the `.p8` file. |

The release also bumps the Homebrew cask in [JoseValencia125/homebrew-tap](https://github.com/JoseValencia125/homebrew-tap) (version and sha256) with `TAP_DEPLOY_KEY`, the private half of a deploy key with write access to that repository.

Locally, `scripts/build.sh` signs with the first "Developer ID Application" identity in the keychain (`CODESIGN_IDENTITY` picks another) and ad-hoc otherwise; `scripts/notarize.sh "build/Agents Pixel Dungeon.app"` notarizes with those same variables or with a notarytool keychain profile in `NOTARY_PROFILE` (`xcrun notarytool store-credentials`).
