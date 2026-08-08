# Security

## Reporting

Report suspected vulnerabilities privately to the maintainers at
<https://github.com/b1narydt/rust-wallet-toolbox/security/advisories/new>.
Please do not open a public issue for a suspected vulnerability.

This crate is the storage and recovery layer of a wallet. Treat anything that
could cause **silent** data loss, key-material loss, or cross-tenant
disclosure as a security issue even if it is not obviously exploitable — a
backup that reports success while holding nothing is a security failure that
only surfaces on the day it is needed.

## Yanked releases

**`0.2.0` through `0.4.0` are yanked. Do not use them.**

Those versions discarded the authenticated identity (`AuthId`) on part of the
storage surface, so a request could read rows belonging to another tenant.
Fixed in `0.5.0` by scoping the authenticated storage surface (`scope.rs`),
and the whole affected range was yanked from crates.io rather than patched.

If you are on any version below `0.5.0`, upgrade. There is no configuration
that makes the affected range safe.

## Supported versions

| Version | Supported |
|---|---|
| `0.6.x` | yes |
| `0.5.x` | security fixes only |
| `0.2.0`–`0.4.0` | **yanked — unsupported** |
| `< 0.2.0` | no |

## What this crate does and does not defend

**Does.** Tenant scoping on the authenticated storage surface; BRC-39
container encryption (Argon2id + AES-256-GCM) with bounded KDF parameters, so
a hostile container header cannot force an unbounded allocation; atomic
restore, with foreign keys enforced on every SQLite connection; admin-reserved
protocol, basket, and label namespaces refused for non-admin originators.

**Does not.** Plaintext BRC-38 import carries no integrity protection — a
truncated or edited document restores as a successful import. Use BRC-39
containers, which are authenticated, as recovery artifacts; treat bare BRC-38
JSON as trusted-path only.

**The database is key material, not a cache.** `derivationPrefix` is random at
creation and there is no gap-scan recovery for BRC-42. Losing the outputs
table makes UTXOs unspendable even if you still hold every key share. Back up
the store, not just the keys.
