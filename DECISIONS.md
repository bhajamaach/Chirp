# Decisions

Every non-obvious decision and deviation from `AGENT_BUILD_BRIEF.md`: what, why, alternatives.

## D1. Overlay window behaviour uses `objc2` from the start (S0, 21 Oct 2026)

- **What:** `platform/macos/overlay.rs` sets the NSWindow level to `NSScreenSaverWindowLevel` and the collection behaviour to `CanJoinAllSpaces | FullScreenAuxiliary | Stationary | IgnoresCycle`, and calls `setIgnoresMouseEvents(true)`.
- **Why:** Tauri's `visible_on_all_workspaces` alone has a reported bug where the window does not stay above other spaces on macOS (tauri-apps/tauri#11488). The brief allows dropping to `objc2` in that case.
- **Alternatives:** Tauri window API only (rejected: known gap); `NSPanel` non-activating panel via a plugin (not tried, extra dependency).

## D2. Overlay hidden while Idle (S0)

- **What:** The overlay window is hidden in the Idle state and shown for every other state.
- **Why:** The PRD allows "hidden or dim dot" for Idle. Hiding means no cursor polling and near-zero idle CPU. The cursor-follower thread sleeps on a condvar while Idle.
- **Alternatives:** Always-visible dim dot (needs a persistent position and more redraws).

## D3. Moods for states the mascot sheet does not cover (S0)

- **What:** The mascot sheet has four moods (idle, listening, thinking, done). Acting reuses the thinking face plus a lime highlight ring. Confirming uses the listening face in amber. Error uses the sleepy-line face in coral. No text anywhere.
- **Why:** The PRD needs a distinct colour for Confirming and a distinct error look. Colours (amber `#ffc857`, coral `#ff7a6b`) are placeholders until sound and colour design is confirmed (PRD open question).
- **Alternatives:** Draw new faces (needs design input).

## D4. Dev-only tray menu to preview states (S0)

- **What:** The menu-bar menu has a "Show state" submenu that sets any state, and "Quit Chirp".
- **Why:** There is no voice loop yet, so this is the only way to see each overlay state and test the full-screen behaviour. Remove or hide it behind a debug flag once S1 drives the state.

## D5. No main window; menu-bar app only (S0)

- **What:** `tauri.conf.json` declares no windows. The overlay is created in `setup`. Activation policy is `Accessory` (no Dock icon). `index.html` is a placeholder for the later settings window.
- **Why:** Brief calls for a menu-bar app plus overlay.

## D6. Free-tier providers for development (planning)

- **What:** Plan to use Sarvam (Saaras V4 STT, Bulbul TTS) on its ₹100 signup credit and Groq's free tier for the LLM, all behind provider traits.
- **Why:** Free to start; Sarvam supports Hindi-English code-mixing. Groq's free tier caps at about 8,000 tokens per minute, so accessibility-tree prompts must stay small.
- **Alternatives:** Cerebras ($5 trial credit that expires in 30 days), Gemini (free-tier limits unclear and changing).
