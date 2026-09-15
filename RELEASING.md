# Releasing

Releases are built by GitHub Actions and attached to a GitHub Release. The
`install.sh` one-liner resolves the latest release (Gitee first, then GitHub) so
it works once at least one of them has a release with the expected assets.

## Asset contract

Each release must include, at minimum:

```
raemote-darwin-arm64.tar.gz
raemote-darwin-x86_64.tar.gz
raemote-linux-x86_64.tar.gz
raemote-linux-arm64.tar.gz
checksums.txt
```

Each tarball contains the two binaries, `raemote` and `raemoted`, at the archive
root. Asset names omit the version so `releases/latest/download/<asset>` works.

## Steps

1. Bump the version in `raemote_server/Cargo.toml` and commit it.
2. Tag and push:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. `.github/workflows/release.yml` runs on `v*` tags: it builds the four
   targets on GitHub runners (macOS 14 for both darwin targets; Ubuntu x86_64
   and Ubuntu arm64 for Linux), packages them, writes `checksums.txt`, and
   attaches everything to the GitHub Release for that tag.

4. Sanity-check the release page: four tarballs + `checksums.txt`.

5. **Mirror the same five assets to a Gitee release** for the tag
   (`pppkin/raemote_server`). The installer prefers Gitee, and GitHub is
   unreliable on some networks (mainland China especially), so a release that
   only exists on GitHub makes the one-liner fall back to a flaky path — or
   fail. Download the assets from the GitHub release and attach them to a Gitee
   release for the same tag:

   ```sh
   gh release download v0.1.0 --dir dist --clobber   # or use scripts/package.sh
   # then attach dist/* to a Gitee release (web UI, or the Gitee API)
   ```

   Gitee release assets don't need re-signing — `checksums.txt` covers the
   tarballs and the installer verifies it whichever source it used.

6. Verify the one-liner on a clean machine (fresh `HOME` is enough):

   ```sh
   curl -fsSL https://github.com/raemote/raemote_server/raw/main/install.sh | sh
   raemote status
   ```

   (If only GitHub has the release, the installer falls back to it
   automatically. Test both entry points when it matters:
   `https://gitee.com/pppkin/raemote_server/raw/main/install.sh`.)

## Manual fallback

No CI is required. On each platform:

```sh
scripts/package.sh                       # host target
scripts/package.sh x86_64-apple-darwin   # cross-build (needs rustup target add)
```

This writes `dist/raemote-<os>-<arch>.tar.gz` and `dist/checksums.txt`. Upload
those files as release assets (Gitee or GitHub).

## Installer resolution order

`install.sh` resolves the latest release in this order:

1. Gitee (`pppkin/raemote_server`), using the Gitee releases API — needs a Gitee
   release with the assets.
2. GitHub (`raemote/raemote_server`), using
   `releases/latest/download/<asset>` — needs a GitHub release.

Override with `--source gitee|github`, `--tag <tag>`, or `--base-url <url>`.

## Pre-release checklist

- [ ] `cargo test` and `cargo clippy --all-targets` are green.
- [ ] iOS builds clean and its tests pass.
- [ ] Version bumped and tagged.
- [ ] Release has all four tarballs + `checksums.txt`.
- [ ] `install.sh` verified against the published release.
- [ ] The manual checklist in [`testing.md`](testing.md) has been run on a real
      device.
