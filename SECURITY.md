# Security policy

## Reporting a vulnerability

Please report security problems privately, never in a public issue, pull request or discussion.

Use GitHub's private vulnerability reporting: open the repository's **Security** tab and choose
**Report a vulnerability**. The report opens a private security advisory that only you and the maintainers can
see. We will discuss the fix there, and publish the advisory (with credit, if you want it) once a fixed version
is available.

We aim to acknowledge a report within a few working days and to keep you informed while we work on it.

## Supported versions

Granary has no release branches yet. Security fixes go to `main` only; please check that a problem still exists
on the latest `main` before you report it.

| Version | Supported |
|---|---|
| `main` | Yes |
| Anything older | No |

## What to include

- The component and version (commit hash) affected: backend API, worker, frontend, a sidecar, or the
  deployment configuration in this repository.
- What an attacker can do, and what access they need first (none, a portal user, an internal user with a
  given role, an MCP connection, network access to a sidecar).
- Steps to reproduce: requests, payloads or a small script, and the configuration they need.
- What you expected to happen and what happened.
- Any idea you have of a fix.

Please test only against your own installation, never against someone else's, and don't access, change or
keep other people's data.

## Scope notes

- Secrets live in `.env` and, encrypted, in the application's settings. A deployment that keeps the
  placeholder values from `.env.example` is misconfigured; Granary logs every placeholder secret it finds at
  start-up and notifies the Owner.
- The sidecars (pdf, face, research, stt, searxng) and the database, Redis and the storage are meant to be reachable
  only on the internal Docker network, behind Nginx. A report that depends on exposing them publicly is still
  welcome, but say so.
