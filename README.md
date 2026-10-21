# Chirp

A voice-first assistant for macOS for blind, low-vision and motor-impaired users. See `CHIRP.md` for what it is and how it is built, `PRD.md` for the product, and `AGENT_BUILD_BRIEF.md` for the v0.1 build plan.

Status: early build. Slices S0 (menu-bar app and overlay) and S1 (permissions, talk key) exist.

## Setup

Requires macOS on Apple Silicon, Rust, Node and pnpm.

```sh
pnpm install
cp .env.example .env   # keys are not needed until slice S2
pnpm tauri dev
```

The first build compiles about 350 crates and takes a few minutes.

## Using the current build

Chirp runs from the menu bar and has no Dock icon. On first launch it speaks, asks for each permission in turn, then asks you to press the key you want to hold to talk. Hold that key and the mascot appears next to your cursor in its Listening state; release it and the mascot goes away.

Menu-bar menu:

- **Talk key:** shows the saved key.
- **Change talk key…:** pick a different key.
- **Check permissions…:** re-check and fix permissions.
- **Show state:** preview each overlay state (development aid).
- **Quit Chirp.**

The saved key and setup state live in `~/Library/Application Support/com.chirp.app/config.json`. Delete that file to repeat first-run setup.

## Required permissions

Chirp asks for these one at a time, with a spoken reason:

- **Microphone:** to hear you (used from S2).
- **Accessibility:** to control other apps, and to swallow the talk key so it does not type.
- **Input Monitoring:** to detect the talk key.
- Screen Recording is only needed if the vision fallback is enabled, which v0.1 does not build.

When you run `pnpm tauri dev`, macOS lists the app that launched it (Terminal, iTerm, VS Code and so on) in these lists instead of Chirp. If Input Monitoring is granted but the key still does nothing, quit and relaunch.

## Choosing a talk key

Right Command, Right Option, Right Control, Right Shift and F1 to F20 work. Typing keys, left-side modifiers, arrow keys, Escape and Caps Lock are refused because holding them would break normal typing. Fn or Globe only works if System Settings, Keyboard has "Press 🌐 key to" set to "Do Nothing".

## Environment variables

See `.env.example`.
