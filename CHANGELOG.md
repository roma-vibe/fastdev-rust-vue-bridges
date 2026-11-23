# Changelog

## 1.1.0 — 2026-11-23

_Not verified on the publishing machine: data=volume, docker=run: Docker >=24; data=local, docker=run: Docker >=24._

- Security headers on every edge response: strict CSP for the built app, default-src 'none' for API responses, HSTS behind https, no-store with credentials.
- Host check at the edge: only localhost, 127.0.0.1, [::1] and the allowed origins' hosts are answered, others get 421 (DNS rebinding guard).
- With accounts on: Origin check on cookie-signed writes, sign-in and registration throttling with growing waits (no lockout), __Host- session cookie behind https, a new token per sign-in.
- Safe outbound HTTP helper ctx.outbound: public addresses only, DNS resolved once, redirects re-checked, 10 s and 5 MB caps.
- Body size limit (MAX_BODY_BYTES, 1 MiB) with 413, generic 5xx bodies, a two-user test proving users never see each other's notes.
- npm run check ends with npm run security (secbase); without secbase it fails, only SECBASE_OPTIONAL=1 skips it with a notice.
- Dependencies updated with no known vulnerabilities: sqlx 0.9 (static SQL strings only), tokio 1.54.1, url 2.5.9, happy-dom 21; Rust 1.94 or newer.
- Docker publishes the edge on 127.0.0.1 only; AGENTS.md and SPEC.md gain a Security section.

## 1.0.0 — 2026-11-09

_Forked from rust-vue-microservices 1.0.0._

- Fork of rust-vue-microservices 1.0.0 with bridges: services are libraries and run in one process by default (BRIDGES=on), about 7 MB instead of about 85 MB.
- BRIDGES=off or SEPARATE_SERVICES run services as separate processes over HTTP with no code changes; dev, start and Docker follow the same rule.
- New app crate links the services into one binary; works with Accounts on and off (per-feature Cargo.toml and Cargo.lock).
- Docs: docs/bridges.md describes the rules a service must follow to stay movable; tests cover all-in-one, mixed and fully separate modes.
