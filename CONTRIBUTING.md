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
