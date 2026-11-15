# Releasing the Rust binary

The Rust build is intended to be installed as a **prebuilt binary**, not through
`uv`, Python, a virtualenv, or a local Rust toolchain.

## Beta stage (unsigned)

For the first beta, keep the release cheap and reversible:

- Publish `tar.gz`/`zip` archives with SHA-256 files.
- Keep the install scripts version-pinnable (`ECLASS_VERSION=rust-v0.2.0`).
- State clearly in the release notes that macOS/Windows binaries are unsigned.
- OZ PDF export uses an installed Chromium browser.
- Windows may show SmartScreen; macOS may show a Gatekeeper warning. Tell users
  how to verify the SHA-256 and how to proceed.

Do not block the beta on Apple Developer/Windows code-signing costs. Add
signing after there are real users.

## Release artifacts

`.github/workflows/rust-release.yml` builds and uploads:

| Platform | Target |
|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux arm64 | `aarch64-unknown-linux-gnu` |
| Windows x64 | `x86_64-pc-windows-msvc` |

Each release contains `eclass-mcp-<target>.tar.gz` (or `.zip`) and a matching
`.sha256`. Each archive includes documentation and license files. The Linux
archives target glibc 2.35 or later.

Create a release manually from **Actions → Rust release**, with a tag such as
`rust-v0.2.0`. The workflow builds the matrix, checksums the archives, and
creates the GitHub Release at the workflow commit. The tag must match the
Cargo package version. A regular Rust release becomes GitHub's latest release.
To install a prerelease, set `ECLASS_VERSION` to its exact tag.

## User installation

macOS/Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | bash
```

Pin a version when needed:

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/rust-v0.2.0/rust/install.sh | ECLASS_VERSION=rust-v0.2.0 bash
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.ps1 | iex
```

Both scripts verify SHA-256 and install to `~/.local/bin` (or
`%LOCALAPPDATA%\eclass-mcp\bin`) by default. They print an onboarding command
that uses the installed executable's full path.

## Update path

There is no self-updater in the beta. Update by re-running the install script
with the new `ECLASS_VERSION` (or the default latest). The session file and
downloads are preserved because they live outside the install directory.
A future `eclass-mcp update --check` can be added without changing the session
format.

## Signing and notarization (after beta)

When usage justifies the cost:

- macOS: sign with a Developer ID Application certificate, then notarize with
  `notarytool`; staple the ticket.
- Windows: sign `eclass-mcp.exe` with an Authenticode certificate.
- Keep signing credentials in GitHub Actions secrets, never in the repository.
- Remove the unsigned-status warning from the release notes only after signed
  artifacts are published.

## Homebrew (optional)

A tap is convenient for macOS users:

```sh
brew tap givemethatsewon/cau-eclass
brew install eclass-mcp
```

The formula should download the Apple Silicon release archive, verify its
SHA-256, and install the `eclass-mcp` binary. This is optional; the install
script is the zero-dependency path.

## Versioning

`rust/Cargo.toml` is the MCP version. Releases use `rust-vX.Y.Z` tags.

## First run

After installation:

```sh
eclass-mcp onboard --client claude
```

`onboard` opens the dedicated login browser, saves the session, checks the
profile, LearningX token, and visible courses, and prints the structured status
documented in [ONBOARDING.md](ONBOARDING.md).
