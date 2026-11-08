# Security Policy

## Supported versions

The `0.1.x` series is currently maintained. Security fixes land in the latest release first.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use the repository's “Security → Report a vulnerability” flow (GitHub Security Advisories) to report privately. Include:

- Affected versions and platforms
- Reproduction steps or a minimal proof of concept
- Impact assessment (for example: approval bypass, workspace escape, key leakage, unbounded spending)
- Any disclosure timeline you would like us to follow

We will acknowledge reports within 7 days and coordinate disclosure once a fix is available.

## Security model (summary)

phi-agent is an agent that can edit files and run shell commands. Its current boundaries include:

- **Deny by default** — unknown tools and paths outside the workspace are refused.
- **Human approval** — writes and command execution ask by default; approvals bind an action digest and are re-checked before execution.
- **Workspace isolation** — file tools stay inside the configured workspace; cancellation terminates the process tree.
- **Dangerous command filtering** — common destructive command patterns are denied outright.
- **Hard budget caps** — step, cost, and wall-clock limits; cost is reserved before each call.
- **Untrusted input** — tool results, search results, and web content do not gain privileges.
- **Audit trail** — policy decisions and tool calls are persisted as local events.

## Usage guidance

- Do not use a directory containing secrets, credentials, or sensitive data as the workspace; reads inside the workspace are allowed automatically.
- Read the approval card's argument preview carefully; execution binds the full argument digest.
- The API key is kept in memory only and is never written to disk.

## Known limitations

- The sandbox is a path jail plus process termination, not OS-level isolation.
- After cancellation, side effects may already have occurred; such results are marked unknown and require manual review.
