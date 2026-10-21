# Chirp: what it is and how it is built

*Living document. Updated as the build progresses. Last updated: 21 Oct 2026 (before any code; everything below is the plan, not yet built).*

## 1. What Chirp is

Chirp is a voice-first assistant for macOS. A user presses and holds a key they chose, says what they want in Hindi, English or a mix, and Chirp does it, narrates what it is doing, and asks for a spoken yes before anything risky. It is built for blind, low-vision and motor-impaired users, so it must work without looking at the screen.

The mascot is a minimal lime squircle with dot eyes and one antenna dot. It has four moods (idle, listening, thinking, done), drawn with dots and shapes, never text. Spoken lines are 12 words or fewer, warm and short, never jokey in errors or confirmations.

Related docs: `PRD.md` (product), `AGENT_BUILD_BRIEF.md` (v0.1 build instructions), `RESEARCH.md` (viability research), `DECISIONS.md` (decision log, created when building starts).

## 2. Methodology

- **Vertical slices, in order (S0 to S7).** Each slice is finished and tested before the next starts, and committed once it works.
- **Build over plan.** One-page plan, then code. No long design documents.
- **Check docs before relying on any crate, API or OS behaviour.** No assumptions from memory.
- **Honest reporting.** Failures are stated plainly. Test passes and measurements are never faked.
- **Measure latency at every stage** (key down, end of speech, STT final, intent ready, first action, first spoken word) and report the numbers even when they miss targets.
- **Safety first.** Irreversible actions never run without an explicit spoken yes. Development uses fake data only.
- **Record non-obvious decisions** in `DECISIONS.md`: what, why, alternatives.
- **No secrets in git.** Keys come from environment variables or a git-ignored `.env`.

## 3. Architecture

Stack: Tauri 2 shell and UI, Rust core with a single async runtime, minimal TypeScript front end.

### Pipeline

```text
hold key -> mic capture -> local VAD -> streaming STT -> intent (fast path or LLM)
         -> safety gate (risk + confirmation) -> executor (layer 0 / layer 1)
         -> narration (streaming TTS) -> action log
```

The state machine (Idle, Listening, Thinking, Acting, Confirming, Done or Error) is the single source of truth. The overlay and the sound cues are driven only by this state.

### Modules

```text
src-tauri/src/
  core/      agent loop, state machine, config
  platform/  PlatformAdapter trait + macos/ implementation
  voice/     audio capture, VAD, stt/, tts/, earcons
  llm/       provider trait, router, planner, schemas
  safety/    risk classifier, confirmation gate, action log
  evals/     task suite and runner
ui/          overlay, settings, key picker
```

### Key interfaces

- **PlatformAdapter:** `frontmost_app()`, `get_ui_tree(depth)`, `find_element(query)`, `perform(element, action)`, `type_text(text)`, `press_keys(keys)`, `focus_app(name)`, `run_os_script(script)`, `screenshot()` (stub in v0.1). macOS implements it now, Windows later.
- **Providers (config, never hard-coded):** one trait each for speech-to-text (streaming over WebSocket, Hindi-English code-mixed), text-to-speech (streaming, Hindi and English voices) and LLM (OpenAI-compatible chat endpoint with JSON-schema output). Selected by environment variables, with retry and backoff and optional LLM failover.
- **Intent schema** (the LLM must return exactly this): `goal`, `app`, `params`, `risk` (safe, reversible, irreversible), `needs_clarification`, `question`. Invalid JSON gets one retry, then a spoken fallback.

### Execution layers (v0.1 builds 0 and 1 only)

0. OS scripting and app intents (AppleScript).
1. macOS accessibility tree.
2. Browser automation (not built, interface only).
3. Vision fallback (not built, interface only).

### Safety

Risk levels are safe, reversible and irreversible. Irreversible (send, pay, delete, submit, overwrite) always restates the action and waits for a spoken yes. Blocklist by default: payments, passwords, system settings. The action log stores actions, not content, and can be read aloud ("what did you just do?"). No screenshots or screen text are stored by default.

### Screen reader coexistence

Chirp must never talk over the user's screen reader and must hand control back when a task fails. The exact mechanism (speak through the screen reader versus its own voice) is an open decision.

## 4. Build status

| Slice | Scope | Status |
|---|---|---|
| Setup | git init, `.gitignore`, Tauri scaffold, `DECISIONS.md`, README | Done |
| S0 | Menu-bar icon, always-on-top click-through overlay | Accepted by the user on 21 Oct 2026 after manual testing. Window level (1000) and tray state menu confirmed working |
| S1 | Key picker, global press-and-hold, spoken permissions flow | Built. Compiles with no warnings and 7 unit tests pass (key rules, config). Not yet verified on a real machine: needs the permission grants, a key pick, and a hold-to-talk test |
| S2 | Audio capture, VAD, streaming STT | Not started |
| S3 | Streaming TTS, sound cues, "stop" interrupt | Not started |
| S4 | macOS adapter: AX tree, find, press, type | Not started |
| S5 | Intent parsing, planner loop, fast path, narration | Not started |
| S6 | Risk classifier, spoken confirm, action log | Not started |
| S7 | Eval harness and results | Not started |

## 5. What exists in code

- `src-tauri/src/core/state.rs`: the `State` enum (Idle, Listening, Thinking, Acting, Confirming, Done, Error) and `StateHub`, a shared state with a condvar. Setting a state emits the `chirp://state` event to the overlay.
- `src-tauri/src/platform/macos/overlay.rs`: native NSWindow tweaks (screen-saver window level, all spaces, full-screen auxiliary, ignores mouse events).
- `src-tauri/src/lib.rs`: builds the transparent, undecorated, always-on-top overlay window, the menu-bar tray (with a dev-only "Show state" submenu), and a thread that moves the overlay beside the cursor while Chirp is not Idle.
- `overlay.html`, `src/overlay.ts`, `src/overlay.css`: the mascot as inline SVG. Each state is a CSS `data-state` rule with its own animation loop, and the TypeScript only sets the state and replays the entry pop. Motion is disabled under the system reduce-motion setting.
- `assets/` and `src-tauri/icons/`: the Chirp app icon and the menu-bar template icon (mascot silhouette), generated from `assets/*.svg`.
- `src-tauri/src/core/overlay.rs`: overlay window, cursor follower and `apply_state` (moved out of `lib.rs`).
- `src-tauri/src/core/config.rs`: the saved config (talk key and setup-complete flag), written atomically.
- `src-tauri/src/core/setup.rs`: first-run flow and menu actions. Walks through Microphone, Accessibility and Input Monitoring one at a time with a spoken reason, installs the key tap, then runs the key picker. One flow runs at a time.
- `src-tauri/src/platform/macos/permissions.rs`: permission checks and requests (AVFoundation for the microphone, `AXIsProcessTrusted`, `CGPreflightListenEventAccess`).
- `src-tauri/src/platform/macos/hotkey.rs`: the global key capture. An event tap on its own thread reports press and release of the talk key, and `pick` waits for the next key press for the picker. State changes are handed to a worker thread so the tap callback stays fast.
- `src-tauri/src/platform/macos/keycodes.rs`: key names, modifier flag bits, the allow-list of keys that may be the talk key, and the Fn check.
- `src-tauri/src/voice/speak.rs`: speech via macOS `say` until the real TTS arrives in S3.
- Tray menu: shows the current talk key, "Change talk key…", "Check permissions…", the dev-only state picker, and Quit.
- App runs as a menu-bar accessory (no Dock icon) and declares no main window.

### How hold-to-talk works

```text
key event -> event tap thread -> is it the talk key? -> press/release channel
          -> worker thread -> state machine (Listening / Idle) -> overlay
```

Holding the talk key sets the state to Listening and releasing it returns to Idle. In S2 the release will go to Thinking instead.

## 6. Decisions and changes so far

Full list with alternatives in `DECISIONS.md`. Summary:

- 21 Oct 2026: project documentation is kept as Markdown files in the repo (`RESEARCH.md`, this file), not Claude Docs artifacts.
- 21 Oct 2026: viability research concluded the build is worthwhile as a prototype. Interview users early, and decide early whether Windows and NVDA should come alongside macOS. See `RESEARCH.md`.

- 21 Oct 2026: overlay window behaviour goes through `objc2` from the start, because Tauri's all-workspaces option has a reported bug (D1).
- 21 Oct 2026: overlay is hidden while Idle so idle CPU stays near zero (D2).
- 21 Oct 2026: development providers will be Sarvam (speech) and Groq (LLM) on free tiers (D6).

## 7. Changelog

| Date | Change |
|---|---|
| 21 Oct 2026 | Created this file from the PRD, build brief and research. |
| 21 Oct 2026 | Scaffolded Tauri 2 app, built S0 (tray, overlay, state machine, mascot states). |
| 21 Oct 2026 | Replaced Tauri default icons with Chirp's app icon and a menu-bar template icon; product name set to "Chirp". |
| 21 Oct 2026 | Added per-state mascot animation and an entry pop; overlay window enlarged to 140 px. S0 accepted by the user. |
| 21 Oct 2026 | Built S1: permissions flow, key picker, saved config, global hold-to-talk. Compiles clean, 7 unit tests pass; real-machine verification pending. |
