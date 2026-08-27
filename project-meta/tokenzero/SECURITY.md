# Security Policy

## Supported Versions

The public release branch supports the current `1.x` line. Older pre-1.0 builds are not supported.

## Reporting a Vulnerability

Use GitHub security advisories if enabled for this repository. If advisories are not available, open a minimal public issue asking for a secure contact path. Do not include exploit details, secrets, tokens, local paths, or sensitive payloads in a public issue.

TokenZero treats the following as security-sensitive:
- Secret or token disclosure.
- Recovery handle expansion across project boundaries.
- Cache reads outside configured roots.
- Local filesystem path disclosure in public artifacts.
- Raw payload leakage through public artifacts.

Expected initial response target: 7 days after a security report is received.
