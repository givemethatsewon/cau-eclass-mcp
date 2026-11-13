# Agent onboarding contract

`eclass-mcp onboard` and the MCP tool `get_onboarding_status` share one JSON
shape. The agent branches on `next_actions[].code`, not on prose. The schema is
versioned by `schema_version` (currently `1`).

## Status shape

```json
{
  "schema_version": 1,
  "session_state": "missing | expired | unavailable | authenticated",
  "authenticated": false,
  "canvas_ok": false,
  "learningx_token": false,
  "learningx_ok": false,
  "learningx_token_age_secs": null,
  "courses_visible": null,
  "ready": false,
  "next_actions": [
    {
      "code": "RUN_ONBOARD",
      "message": "Sign in to e-Class once in the dedicated browser.",
      "command": "eclass-mcp onboard --client <client>"
    }
  ],
  "session_file": "/Users/.../session.json",
  "error": null
}
```

`learningx_token` is a boolean. **Cookie values, bearer tokens, and session
file contents are never returned.** A regression test serializes the status and
asserts that the synthetic token and cookie name do not appear.

## Action codes

| Code | Meaning | Typical next step |
|---|---|---|
| `RUN_ONBOARD` | No saved session | Ask the user to run `eclass-mcp onboard --client <client>` |
| `SESSION_EXPIRED` | Session exists but is no longer accepted | Ask for `eclass-mcp login` |
| `CHECK_CONNECTION` | Session could not be verified (network/profile) | Run `eclass-mcp doctor`, retry |
| `REFRESH_LEARNINGX_TOKEN` | Canvas session works, LearningX token missing | `eclass-mcp auth refresh --via aside` mints it from the LTI launch or the signed-in Aside browser |
| `CHECK_ENROLLMENT` | Course list failed or is empty | Ask the user to verify enrollment/term or re-login |
| `CONFIGURE_CLIENT` | Ready | Run `eclass-mcp config --client <client>` and restart MCP |

## Agent flow

1. Call `get_onboarding_status`.
2. If `session_state` is `missing`/`expired`, ask the user to run the command
   from the corresponding action. Never ask for a password, OTP, cookie, or
   session-file contents in chat.
3. Re-call `get_onboarding_status` after the user confirms. Do not infer
   success from a command exiting.
4. If `REFRESH_LEARNINGX_TOKEN` remains, run
   `eclass-mcp auth refresh --via auto`. It first replays the LTI launch with
   the saved Canvas cookies, then falls back to the signed-in Aside browser.
5. If `ready=true`, call `eclass-mcp config --client <client>` and tell the
   user to restart the MCP connection.
6. Read-only tools are the default. Submission remains a separate explicit
   opt-in; an onboarding instruction is not submission permission.

## LearningX automation

LearningX issues `xn_api_token` from an LTI launch on a course tool page. The
Rust core reads the course page, finds the board/module tool, parses the
`tool_form`, and posts it with the saved Canvas cookies. No browser is needed
for this step. `eclass-mcp auth refresh --via http` runs exactly that flow.

When the Canvas cookies are not usable, `--via aside` uses the signed-in Aside
browser to read the token from the user's existing e-Class session. This avoids
a second login and a dedicated profile.

If neither path succeeds, the status keeps `REFRESH_LEARNINGX_TOKEN` and the
agent asks the user to run `eclass-mcp login`.

## Session storage

- macOS: `~/Library/Application Support/cau-eclass/session.json`
- Linux: `${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json`
- Windows: `%APPDATA%\cau-eclass\session.json`
- Override: `ECLASS_SESSION_FILE`

The file contains only `.cau.ac.kr` cookies, is written `0600` (directory
`0700`), and is deleted by `eclass-mcp logout`. It is a bearer credential:
never sync or share it. OS keychain storage is a possible future enhancement;
until then the owner-only file is the minimum condition.

## Versioning

Add fields in a backward-compatible way and bump `schema_version` for a
breaking change. Keep action codes stable; add new codes rather than renaming
existing ones.
