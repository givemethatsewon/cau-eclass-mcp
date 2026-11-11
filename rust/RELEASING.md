# Releasing the Rust binary

The Rust build is intended to be installed as a **prebuilt binary**, not through
`uv`, Python, a virtualenv, or a local Rust toolchain.

## Beta stage (unsigned)

For the first beta, keep the release cheap and reversible:

- Publish `tar.gz`/`zip` archives with SHA-256 files.
- Keep the install scripts version-pinnable (`ECLASS_VERSION=rust-v0.1.0`).
- State clearly in the release notes that macOS/Windows binaries are unsigned.
- Keep the Python `uv` path available as a fallback:
  `uv tool install --python 3.12 'cau-eclass-mcp[cdp]==0.4.0'`.
- Windows may show SmartScreen; macOS may show a Gatekeeper warning. Tell users
  how to verify the SHA-256 and how to proceed.

Do not block the beta on Apple Developer/Windows code-signing costs. Add
signing after there are real users.

## Release artifacts

`.github/workflows/rust-release.yml` builds and uploads:

| Platform | Target |
|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux arm64 | `aarch64-unknown-linux-gnu` |
| Windows x64 | `x86_64-pc-windows-msvc` |

Each release contains `eclass-mcp-<target>.tar.gz` (or `.zip`), a matching
`.sha256`, `README.md`, `PORTING.md`, and `ONBOARDING.md`.

Create a release manually from **Actions → Rust release**, with a tag such as
`rust-v0.1.0`. The workflow builds the matrix, checksums the archives, and
creates the GitHub Release. GitHub occasionally retires runner labels; update
the matrix in `.github/workflows/rust-release.yml` if a label stops resolving.

## User installation

macOS/Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | sh
```

Pin a version when needed:

```sh
ECLASS_VERSION=rust-v0.1.0 curl -fsSL .../rust/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.ps1 | iex
```

Both scripts verify SHA-256 and install to `~/.local/bin` (or
`%LOCALAPPDATA%\eclass-mcp\bin`) by default. Users do not need Rust.

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

The formula should download the arm64/x86_64 release archive, verify its
SHA-256, and install the `eclass-mcp` binary. This is optional; the install
script is the zero-dependency path.

## Versioning

`rust/Cargo.toml` is versioned independently from the Python package until the
Rust build reaches parity. Use `rust-vX.Y.Z` tags to avoid colliding with the
Python `vX.Y.Z` release workflow.

## First run

After installation:

```sh
eclass-mcp onboard --client claude
```

`onboard` opens the dedicated login browser, saves the session, checks the
profile, LearningX token, and visible courses, and prints the structured status
documented in [ONBOARDING.md](ONBOARDING.md).
