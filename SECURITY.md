# Security Policy

Raemote runs inside your home and has access to applications on your machine, so
we take security reports seriously and appreciate responsible disclosure.

## Supported versions

The latest released version receives security fixes. Pre-1.0 versions are
supported on a best-effort basis.

| Version | Supported |
| --- | --- |
| 0.1.x (latest) | Yes |
| older | No |

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report privately to:

> **cool@lyuhj.top**

If that address is unavailable, open a minimal public issue that says only
"security contact requested" (no details) and we will reach out.

Please include, where possible:

- what you were doing and what you expected;
- the impact (what an attacker gains);
- steps to reproduce, or a proof of concept;
- affected version (`raemote --version`) and platform;
- any suggested fix.

## What to expect

- Acknowledgement within a few days.
- An assessment and, if confirmed, a fix and coordinated disclosure.
- Credit in the release notes if you would like it.

We aim to fix confirmed issues before public disclosure and will agree on a
timeline with you. Please give us reasonable time before disclosing publicly.

## Scope

In scope:

- the `raemote` server (daemon + CLI);
- the Raemote iOS app;
- the pairing and device-trust model.

Out of scope:

- vulnerabilities in the applications Raemote proxies (report those to their
  authors);
- issues that require an already-compromised device or root access on the host;
- denial of service from an attacker who is already a paired device.

See [`docs/threat-model.md`](docs/threat-model.md) for the security model and its
known limitations.
