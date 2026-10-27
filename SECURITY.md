# Security policy

## Reporting a vulnerability

Please **don't** report security issues in public issues, discussions or pull requests.

Report them privately through GitHub:
**[Report a vulnerability](https://github.com/dokyit/Trek/security/advisories/new)**
(the repository's **Security** tab › **Report a vulnerability**). Only the maintainers see it.

Helpful to include:

- What an attacker could do, and what they need first (local access, the same network, a paired device…)
- Steps to reproduce, or a proof of concept
- Trek version (Settings › About), platform and OS version

We'll acknowledge the report, keep you updated while we work on a fix, and credit you in the
advisory unless you'd rather stay anonymous. Please give us a reasonable time to ship a fix
before you disclose anything publicly.

## Supported versions

| Version | Supported |
| --- | --- |
| Latest stable release | Yes |
| Latest beta or nightly | Best effort |
| Anything older | No: update first (Settings › Updates) |

## Areas we care most about

- **Updates**: the signature check on downloaded updates (minisign) and the install step.
- **Phone and machine pairing**: pairing codes, device tokens and certificate pinning
  ([docs/MOBILE.md](docs/MOBILE.md)).
- **Agent permissions**: anything that lets an agent run a command, edit a file or use the
  computer without the approval its hand-holding level requires.
- **Credentials**: API keys in the macOS Keychain or Windows Credential Manager.
- **Trek's MCP server and local socket** (`trek-mcp`, `trek-ipc`).

Vulnerabilities in the agent CLIs Trek runs (Claude Code, Codex, …) belong with their makers.
