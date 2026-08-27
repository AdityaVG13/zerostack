# Security Policy

## Reporting a vulnerability

Use GitHub Security Advisories for private reporting:

- Go to **Security → Report a vulnerability** on https://github.com/AdityaVG13/fszero
- Or open `https://github.com/AdityaVG13/fszero/security/advisories/new`

Do not open a public GitHub Issue for a suspected vulnerability.

Include a description, impact, reproduction steps or proof of concept, and any affected versions or commits if known.

## Scope

This policy covers the FSZero repository at `AdityaVG13/fszero`. Adjacent engines (GraphZero, TokenZero) and the ZeroStack hub have their own repositories and policies.

FSZero treats the following as security-sensitive:

- Reads or writes outside configured store roots
- CAS hash mismatch accepted as content
- Overlay / journal restore that mutates the wrong tree
- Secret or token disclosure in public artifacts
- Local filesystem path disclosure in public artifacts
