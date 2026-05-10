# Notices

## Acknowledgments

VRCWatchDog is **inspired by** [VRCTimeline](https://github.com/tsugu233/VRCTimeline)
by [@tsugu233](https://github.com/tsugu233), released under the MIT License
(Copyright (c) 2026 tsugu233).

VRCWatchDog is a complete from-scratch reimplementation in **Rust + Tauri v2 +
SvelteKit**. No source code is copied or derived from VRCTimeline; only the
problem framing, the architectural lessons, and the concrete issues
(#1–#15 in the original repository) that motivated the rewrite carry over.

The independent licensing of VRCWatchDog (see [`LICENSE`](LICENSE), MIT,
Copyright (c) 2026 kqnade) is a project-level choice; it happens to be MIT-
compatible with the original but is not an inheritance.

## Third-party dependencies

VRCWatchDog redistributes binaries linked against numerous open-source crates
(see `Cargo.lock`) and npm packages (see `crates/app/frontend/pnpm-lock.yaml`).
The respective licenses (predominantly MIT and Apache-2.0) are bundled with
each dependency under `target/` and `node_modules/` at build time and shipped
inside the NSIS installer in accordance with their terms.
