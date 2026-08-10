# Security Policy

grpc-php-rs handles TLS, credentials, and untrusted network input inside your
PHP process — security reports are taken seriously and handled promptly.

## Supported Versions

| Version | Supported |
|---|---|
| latest release | ✔️ |
| older releases | ❌ (upgrade first) |

## Reporting a Vulnerability

**Do not open a public issue.** Report privately via
[GitHub Security Advisories](https://github.com/BSN4/grpc-php-rs/security/advisories/new)
or by email to **baderbsn@proton.me**.

You will get an acknowledgment within 72 hours. There is no bug bounty, but
reporters are credited in the release notes unless they prefer otherwise.

## Scope

In scope — bugs in this repository's source:

- TLS verification or downgrade issues (e.g. a channel silently connecting
  without the credentials the caller supplied)
- Credential handling (CallCredentials tokens sent where they shouldn't be)
- Memory safety, crashes, or data corruption triggerable by a remote peer
- Metadata/payload handling that corrupts or leaks data across calls or
  threads (ZTS)

Out of scope:

- Vulnerabilities in dependencies (tonic, rustls, tokio, …) — report upstream;
  a PR bumping the dependency here is welcome
- Deliberately insecure configuration (`createInsecure()` is insecure by
  design)
- The checked-in PKI under `tests/server/tls/` — a test-only fixture with
  intentionally published keys; it secures nothing
- Bugs in the official C `ext-grpc` (this project is a replacement for it)
