# Chirp

A voice-first assistant for macOS for blind, low-vision and motor-impaired users. See `CHIRP.md` for what it is and how it is built, `PRD.md` for the product, and `AGENT_BUILD_BRIEF.md` for the v0.1 build plan.

Status: early build. Only slice S0 (menu-bar app and overlay) exists.

## Setup

Requires macOS on Apple Silicon, Rust, Node and pnpm.

```sh
pnpm install
cp .env.example .env   # keys are not needed until slice S2
pnpm tauri dev
```

The first build compiles about 350 crates and takes a few minutes.

## Using the S0 build

Chirp runs from the menu bar and has no Dock icon. Use the menu-bar icon, then "Show state", to preview each overlay state. "Quit Chirp" exits.

## Required permissions

None yet. Later slices will ask, one at a time with a spoken reason: Microphone, Accessibility, Input Monitoring, and Screen Recording only if the vision fallback is enabled.

## Environment variables

See `.env.example`.
