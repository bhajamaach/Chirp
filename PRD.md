# PRD v0.1: Voice-First Computer Control

*Name: Chirp · Draft v0.1 · 21 Oct 2026 · Status: for review*

## 1. Summary

A key-press-activated assistant that lets blind, low-vision and motor-impaired people operate their computer by describing what they want done. It understands intent with LLMs, executes through the fastest reliable layer available (OS scripting, accessibility APIs, browser automation, vision as a last resort), narrates progress aloud, and asks before anything risky. The interface is a small cursor-companion overlay with audio cues, so it works without looking at the screen.

## 2. Problem and opportunity

- Built-in voice control tools are command-based: the user must learn a grammar and drive the interface step by step.
- Capable computer-use models now exist, but they are generic, screenshot-heavy and slow, with no voice-first, safety-first layer designed for accessibility.
- **Opportunity:** intent-level voice control that is fast, narrated and safe, built for a specific user group rather than "everyone".

## 3. Target users (hypothesis; validate with 2 to 3 interviews)

- Primary: blind and low-vision users who want to get everyday tasks done without navigating element by element.
- Secondary: people with motor impairments affecting mouse and keyboard use.
- Everyone else can use it, but only the users above drive requirements.

## 4. Goals and non-goals

**Goals**

- Complete 5 core tasks reliably by voice on macOS, in both Hindi and English (including mixed Hindi-English speech).
- Feel fast: spoken acknowledgement almost immediately, first action within seconds.
- Never take an irreversible action without confirmation.
- Model-agnostic: any model can be swapped behind one interface.

**Non-goals (v1)**

- A general "do anything" assistant.
- Windows and Linux (adapter interface only).
- Training or fine-tuning custom models, or local-only model support (v1 uses cloud models).
- Multi-user accounts or cloud sync.

## 5. Core tasks (general everyday use; finalise the tested set after user interviews)

| # | Task | Example utterance | Primary layer |
|---|---|---|---|
| 1 | Open and control a native app | "Open Notes and write a shopping list" | Accessibility |
| 2 | Email triage and reply | "Reply to Rahul and say I'll be late" | Browser or accessibility |
| 3 | Web search and read-back | "Find the cheapest train to Delhi tomorrow" | Browser |
| 4 | File management | "Move my latest PDF to Documents" | OS scripting |
| 5 | Calendar event | "Set a meeting with Priya on Friday at 4" | Browser or accessibility |

Positioning is general everyday use, like a sighted user's normal computing. These five tasks are the tested core. Anything outside them is attempted, and when it fails the assistant says so and hands control back to the user's screen reader.

## 6. Functional requirements

### 6.1 Voice loop

- Activation: on first run the app asks the user to press the key they want, saves it, and uses press-and-hold to talk; the key can be changed in settings. A hands-free wake word is a later option for users who cannot press keys.
- Local voice-activity detection for end-of-speech.
- Streaming speech-to-text; streaming text-to-speech that starts on the first tokens.
- Interruptible: the user can say "stop" at any moment. It coexists with the user's screen reader: it never talks over it and hands control back to it when a task fails.

### 6.2 Intent understanding (LLM)

- Convert speech into a structured intent: goal, target app or site, parameters, risk level.
- Ask at most one clarifying question, and only when the intent is ambiguous.
- Fast path: simple commands (scroll, open app, go back) resolved by rules or a small local model with no LLM round trip.

### 6.3 Execution layers (try in order)

0. OS scripting and app intents
1. Accessibility tree (structured UI, no vision)
2. Browser automation in the user's own browser (already logged in)
3. Vision fallback (screenshot, grounding model, coordinates)

### 6.4 Safety and trust

- Risk levels: *safe* (read-only), *reversible*, *irreversible* (send, pay, delete, submit).
- Irreversible actions require explicit spoken confirmation that restates what will happen.
- Full action log, viewable and readable aloud.
- Undo where the OS or app supports it.
- Hard blocklist: payments, passwords, system settings, unless explicitly enabled.

### 6.5 Privacy

- Show clearly what data leaves the device (transcripts, UI tree, screenshots).
- v1 uses cloud models for speech and language; a local-only mode is deferred.
- No screenshots stored by default.

## 7. Interface (cursor companion)

Audio-first: every state has a sound and a spoken cue, so nothing depends on sight. A small animated overlay near the cursor serves low-vision and sighted users, with large, high-contrast, adjustable styling.

| State | Visual | Audio |
|---|---|---|
| Idle | Hidden or dim dot | None |
| Listening | Pulsing ring | Short rising tone |
| Thinking | Slow shimmer | Soft tick |
| Acting | Highlight ring on the target element | Brief spoken narration |
| Confirming | Distinct colour, shows the planned action | Spoken question |
| Done or error | Flash | Success or error tone plus summary |

Settings: voice, speaking speed, verbosity (terse or detailed narration), wake word, language.

**Always on screen.** The overlay sits above all other windows, including full-screen apps and every desktop space, and never takes focus or blocks clicks.

**Permissions, asked one at a time on first run with a spoken explanation:**

- Microphone
- Accessibility (control other apps and read their UI)
- Input Monitoring (detect the chosen key)
- Screen Recording (only if the vision fallback is enabled)

Drawing the overlay needs no permission on macOS. It cannot draw over secure system screens such as the lock screen or password prompts.

## 8. Non-functional requirements (proposed targets; validate by measurement)

| Metric | Target |
|---|---|
| End of speech to first spoken acknowledgement | under 700 ms |
| End of speech to first action, fast path | under 1.5 s |
| End of speech to first action, LLM path | under 3 s |
| Task success on the 5 core tasks | 90% or better over 20 runs each |
| Idle resource use | negligible CPU, low memory |

## 9. LLM functionality

- **Model tiers:** (a) a small fast router for intent and simple actions, (b) a stronger planner for multi-step tasks and recovery, (c) a vision grounding model as a fallback.
- **Provider interface:** one abstraction for chat, structured output and vision, so models are swappable by configuration.
- **Structured outputs:** all model output is validated JSON; invalid output triggers a retry or a spoken fallback.
- **Replay:** successful action sequences are saved and re-run without the LLM when the screen state matches.
- **Evals:** a fixed task suite run against every model or prompt change, tracking success, latency and cost. Development uses free-tier hosted models behind the same interface (fast open-weight models for routing, a free multimodal tier for vision, an Indic speech provider with free signup credits). Free tiers can log prompts and are rate-limited, so development uses fake data only and the client retries with backoff.

## 10. Tech stack (proposed; confirm at architecture review)

- **Core:** Rust (agent loop, audio, state machine, platform adapters). Fallback if the team lacks Rust: Python core with Rust hot paths later.
- **UI shell and overlay:** Tauri with a web front end (transparent, click-through overlay window).
- **Platform adapters:** macOS accessibility API and event injection first; Windows UI Automation and SendInput behind the same interface.
- **Browser:** DevTools-protocol automation into the user's own browser via a small extension or debugging port.
- **Audio:** local voice-activity detection; streaming speech-to-text and text-to-speech over persistent connections.
- **Speed techniques:** warm connections, parallel pipeline stages, accessibility tree pre-fetch while the user is still speaking, fast path with no LLM, replay cache.

## 11. Evaluation plan

- Success rate and latency per task, over 20 runs each, in Hindi and in English.
- Side-by-side comparison with the OS's built-in voice control on the same tasks.
- Usability test with at least 2 target users.
- Safety review: attempt to trigger irreversible actions without confirmation.

## 12. Milestones

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Spike: accessibility tree dump and click by typed command | Works in 3 apps |
| 1 | Voice core plus layers 0 and 1 | 5 native tasks by voice |
| 2 | Browser layer in the user's browser | 3 web tasks, already logged in |
| 3 | Model tiering, vision fallback, replay | Latency targets met |
| 4 | Safety, logging, privacy controls | No unconfirmed irreversible action |
| 5 | Evaluation and user testing | Report-ready numbers |

## 13. Risks

- **Latency:** agent loops are slow; mitigated by fast path, streaming and replay.
- **Reliability on unusual apps:** mitigated by layered fallbacks and a narrow task set.
- **Activation key:** Fn is not a normal key. macOS uses it for dictation and the emoji picker, and capturing it needs a low-level event tap; Windows keyboards do not expose it to the OS. A first-run key picker avoids this: the user chooses a key that works on their machine.
- **Permissions:** OS accessibility and screen-recording permissions are a setup hurdle for the very users we target.
- **Privacy:** screen and UI data going to cloud models.
- **Scope creep:** the biggest risk; the 5-task list is the guard rail.

## 14. Open questions

- Who exactly are the first 2 to 3 users, and what are their top tasks?
- Which cloud speech and language providers give the best Hindi and mixed Hindi-English accuracy and latency?
- Is the Fn key viable on the target Mac, or do we default to another key?
- Which screen reader do the first users run, and how should the assistant share audio with it?
- Branding is set: the name is Chirp, with a minimal dot-style mascot; confirm the colour and sound design.
