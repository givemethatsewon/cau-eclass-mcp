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
  "learningx_token": false,
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
| `OPEN_WEEKLY_LEARNING` | Canvas session works, LearningX token missing | `onboard`/`login` attempts an automated weekly-tab visit; fall back to the manual step |
| `CHECK_ENROLLMENT` | Course list failed or is empty | Ask the user to verify enrollment/term or re-login |
| `CONFIGURE_CLIENT` | Ready | Run `eclass-mcp config --client <client>` and restart MCP |

## Agent flow

1. Call `get_onboarding_status`.
2. If `session_state` is `missing`/`expired`, ask the user to run the command
   from the corresponding action. Never ask for a password, OTP, cookie, or
   session-file contents in chat.
3. Re-call `get_onboarding_status` after the user confirms. Do not infer
   success from a command exiting.
4. If `OPEN_WEEKLY_LEARNING` remains, ask the user to open **주차학습 (모듈)**
   once in the login browser, then run `eclass-mcp login --no-launch`.
5. If `ready=true`, call `eclass-mcp config --client <client>` and tell the
   user to restart the MCP connection.
6. Read-only tools are the default. Submission remains a separate explicit
   opt-in; an onboarding instruction is not submission permission.

## LearningX automation

`login`/`onboard` first request `/learningx/main` and `/learningx/dashboard`
with the saved cookies. If the API token is still absent, they open
`/learningx/main` in the dedicated Chrome profile, then poll the profile
cookie database for `xn_api_token`. If the browser automation fails, the status
keeps `OPEN_WEEKLY_LEARNING` and the manual fallback is used.

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
