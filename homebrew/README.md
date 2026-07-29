# Homebrew Formula for myc

This directory contains the Homebrew formula for `myc`. To make it installable via
`brew install marcmantei/tap/myc`, the formula needs to live in a dedicated tap
repository on GitHub.

## Setting up the tap repository

1. Create a new GitHub repo named `marcmantei/homebrew-tap`.
2. Copy `myc.rb` into the `Formula/` directory of that repo:
   ```
   homebrew-tap/
     Formula/
       myc.rb
   ```
3. Users can then install with:
   ```bash
   brew tap marcmantei/tap
   brew install myc
   # or in one step:
   brew install marcmantei/tap/myc
   ```

## Updating the formula after a new release

When a new version is released (via the `release.yml` workflow), the formula
needs two updates: the **version** and the **SHA256 checksums**.

### Manual process

1. Download the checksums file from the GitHub Release:
   ```bash
   curl -sL https://github.com/marcmantei/myceliums/releases/download/v0.3.2/checksums-sha256.txt
   ```
2. Update `version` in the formula to the new version.
3. Replace each `sha256 "PLACEHOLDER"` with the corresponding checksum from the file:
   - `myc-aarch64-apple-darwin.tar.gz` -> macOS ARM
   - `myc-x86_64-apple-darwin.tar.gz` -> macOS Intel
   - `myc-aarch64-unknown-linux-gnu.tar.gz` -> Linux ARM
   - `myc-x86_64-unknown-linux-gnu.tar.gz` -> Linux x64

### Automated process

The `update-homebrew.yml` GitHub Actions workflow automatically updates the
formula in the `marcmantei/homebrew-tap` repository whenever a new release is
published. It fetches the checksums from the release assets and commits the
updated formula directly to the tap repo.

For this to work, create a GitHub fine-grained personal access token with
`contents: write` permission on `marcmantei/homebrew-tap`, and add it as a
repository secret named `HOMEBREW_TAP_TOKEN` in the myceliums repo.

## SHA256 placeholders

The formula ships with `sha256 "PLACEHOLDER"` values. These must be replaced
with real checksums before the formula will work. The checksums are generated
by the release workflow and attached to each GitHub Release as
`checksums-sha256.txt`.
