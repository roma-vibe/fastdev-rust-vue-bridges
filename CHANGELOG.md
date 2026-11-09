# Changelog

## 1.0.0 — 2026-11-09

_Forked from rust-vue-microservices 1.0.0._

- Fork of rust-vue-microservices 1.0.0 with bridges: services are libraries and run in one process by default (BRIDGES=on), about 7 MB instead of about 85 MB.
- BRIDGES=off or SEPARATE_SERVICES run services as separate processes over HTTP with no code changes; dev, start and Docker follow the same rule.
- New app crate links the services into one binary; works with Accounts on and off (per-feature Cargo.toml and Cargo.lock).
- Docs: docs/bridges.md describes the rules a service must follow to stay movable; tests cover all-in-one, mixed and fully separate modes.
