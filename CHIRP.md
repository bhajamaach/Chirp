# Chirp: what it is and how it is built

*Living document. Updated as the build progresses. Last updated: 21 Oct 2026 (S6 built).*

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
| S1 | Key picker, global press-and-hold, spoken permissions flow | Accepted by the user after manual testing |
| S2 | Audio capture, VAD, streaming STT | Built and tried on a real voice by the user: hold-to-talk transcribes (flush to transcript 618 to 1228 ms over three presses). Accuracy on the name "Chirp" was poor in that test, so keyterms and a name cleaner were added; their effect on a real voice is not yet confirmed. Hindi and mixed speech verified only with generated clips |
| S3 | Streaming TTS, sound cues, "stop" interrupt | Spoken replies, disk cache, system-voice fallback and sound cues work, and pressing the talk key stops speech in 1 to 9 ms (measured). **The "say stop" part is not met on laptop speakers:** two voice-interrupt designs failed a real acoustic test (the second interrupted Chirp itself in 2 of 4 plays). It stays off by default (`CHIRP_BARGE_IN=1`, untested with headphones) until a proper echo canceller replaces it. Sound cues not yet checked by ear on real use |
| S4 | macOS adapter: AX tree, find, press, type | Built and run against real TextEdit, Finder and Notes from typed commands: 15 steps passed, 0 failed, 1 skipped (`chirp --ax selftest`). The skip is Finder scripting, which macOS has not allowed. Not yet accepted by the user. Caveats: one Mac (macOS 26.5.1), English interface labels, and focus was stolen by another app during two earlier runs (now guarded) |
| S5 | Intent parsing, planner loop, fast path, narration | Built. 133 unit tests pass, including whole tasks run against a fake Mac and a scripted model. Live model calls checked (English, Hindi, a file move): intents and first steps come back valid. **Not yet run end to end by voice or on real apps:** that is for the user to try. The first action lands about 3 to 5.5 s after the transcript on the model path, over the 3 s target |
| S6 | Risk classifier, spoken confirm, action log | Built, with conversation features. 168 unit tests pass (risk rules, yes/no parsing in English and Hindi, the wait loop, action log, and whole tasks asking, being told yes or no, and being interrupted). Live model checked for chat and Hindi chat. **Not yet tried by the user, and no real spoken yes or no has been tested through the microphone** |
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

- `src-tauri/src/voice/audio.rs`: mono downmix, a streaming box-filter resampler to 16 kHz, a 16 ms frame chunker and a WAV wrapper.
- `src-tauri/src/voice/capture.rs`: microphone capture with cpal. The stream exists only while the key is held.
- `src-tauri/src/voice/vad.rs`: local voice detection (Earshot) plus a start/end tracker with hysteresis.
- `src-tauri/src/voice/stt/`: the speech-to-text result and error types, and `sarvam.rs`, the streaming WebSocket client (pure functions for the protocol, so they are testable without credit).
- `src-tauri/src/voice/listen.rs`: one spoken turn. Captures, runs the detector, streams to the provider, flushes on release, records the stage timings and the spend.
- `src-tauri/src/voice/cli.rs`: `chirp --transcribe-file clip.wav` runs a clip through the same path, for testing without speaking and later for evals.
- `src-tauri/src/core/session.rs`: drives the overlay through a turn (Listening, Thinking, Done or Error) and speaks failures.
- `src-tauri/src/core/usage.rs`: the spend estimate and budget guard (seconds only, no content).

- `src-tauri/src/voice/playback.rs`: the single audio output stream and queue for speech and cues. `stop()` empties the queue; the stream pauses when idle. It also keeps a 16 kHz copy of recent output for the interrupt detector.
- `src-tauri/src/voice/earcons.rs` and `src-tauri/src/core/cues.rs`: the synthesised sound cues and the thread that plays one on each state change.
- `src-tauri/src/voice/tts/`: the Sarvam streaming text-to-speech client (`config`, `text`, `flush`; pure protocol functions with tests).
- `src-tauri/src/voice/speaker.rs`: speaks a line from the disk cache, else Sarvam, else the macOS voice. Fixed phrases are cached; text with the user's words never is. Holds the cancel and the "speaking" signal.
- `src-tauri/src/voice/speak.rs`: the small public API (`say`, `say_uncached`, `stop`) the rest of the app calls.
- `src-tauri/src/voice/barge.rs` and `src-tauri/src/core/barge_in.rs`: experimental speak-over-Chirp interrupt, off by default. The gate subtracts one delayed copy of Chirp's output from the microphone and watches what is left. It passes synthetic tests but failed in a real room; see `DECISIONS.md` D26.
- `src-tauri/src/voice/transcript.rs`: removes the user addressing Chirp by name ("hey chair", "Chirp,") from a transcript, without rewriting real words.

- `src-tauri/src/platform/mod.rs`: the `PlatformAdapter` trait and its types (`AppInfo`, `ElementInfo`, `UiNode`, `ElementQuery`, `Action`, `Performed`, `AdapterError`), plus `UiNode::outline`, a short text summary of a window for the planner.
- `src-tauri/src/platform/macos/ax.rs`: thin wrapper over the accessibility C API (batched attribute reads, actions, timeouts).
- `src-tauri/src/platform/macos/adapter.rs`: `MacAdapter`, the macOS implementation. Walks the tree with limits, finds elements by label (exact beats prefix beats word beats substring, shorter wins ties), presses with a fallback ladder, focuses apps with `open -a`, runs AppleScript with a timeout.
- `src-tauri/src/platform/macos/input.rs`: synthetic typing (unicode, so Hindi works) and shortcuts.
- `src-tauri/src/platform/macos/cli.rs` and `selftest.rs`: `chirp --ax <command>` for typed commands, and the S4 acceptance test.

- `src-tauri/src/llm/`: `mod.rs` is the model client (OpenAI-compatible chat, strict JSON schema, retry with backoff, failover between providers, spend tracked for Sarvam). `schema.rs` holds the `Intent` and `Step` types, their JSON schemas, validation, and the prompts.
- `src-tauri/src/core/agent.rs`: the agent. Fast path, then intent, then the planner loop; generic over the adapter, model and narrator so it is tested with mocks. Returns a `Report` with the log of actions (not content), the plan, and timings.
- `src-tauri/src/core/fastpath.rs`: rules for open app, scroll, back, stop, which app. `src-tauri/src/core/narrator.rs`: ordered, cancellable speech. `src-tauri/src/safety/mod.rs`: the first safety gate.
- `src-tauri/src/core/cli.rs`: `chirp --plan "request"` (what the model would do first, touches nothing) and `chirp --run "request" [--speak]` (runs it for real).

### How a request becomes actions (S5)

```text
transcript -> fast path? (open app, scroll, back, stop, which app) -> run it, no model
           -> else: "On it." -> Intent call -> needs a fact? -> ask one question
                                           -> irreversible? -> refuse (until S6)
           -> loop, up to 12 times:
                observe (front app + screen outline) -> model picks plan + one action
                -> safety gate -> run it (focus, press, type, keys, script, wait)
                -> done / ask / fail ends it; 3 failures or a repeat stops it
           -> speak the result; overlay Done or Error; talk key cancels at any step
```

- `src-tauri/src/safety/mod.rs`: `classify` (risk from the action itself), `check` (Refuse, Confirm or Allow) and the wording of the confirmation question. `safety/confirm.rs`: reads a spoken yes or no, and the wait loop. `safety/log.rs`: the action log (`actions.jsonl`), read aloud by rule.
- `src-tauri/src/core/memory.rs`: the short conversation memory.
- `src-tauri/src/voice/phrases.rs` and `voice/lines.rs`: the list of fixed spoken lines, and the recorded-voice-lines loader and importer (`chirp --voice-lines`). A fixed line with a recording is played from it before the synthetic voice is tried.

### How a risky step is confirmed (S6)

```text
model picks a step + its risk -> classify(action) reads the action itself -> the higher risk wins
  refuse (shell, settings, passwords, payments) -> say why, stop
  irreversible (send, delete, move, submit, overwrite) -> "I'll press Send in Mail. Is that okay?"
        user holds the talk key and answers -> yes: run exactly that step, log "after you said yes"
                                            -> no / silence / unclear twice: do not run, "Okay, I'll leave it."
  anything else -> run it
every step run, declined or refused -> action log (actions, not content) -> "what did you just do?"
```

### How speaking and interrupting work

```text
say("text") -> cached recording? -> yes: play (2 to 12 ms to start)
                                 -> no:  Sarvam TTS stream -> play as chunks arrive -> cache if a fixed phrase
                                 -> fail/no key/no credit: macOS voice
talk key down -> Speaker.stop() -> queue emptied, line cancelled, queued lines dropped -> Listening
(experimental, off by default, failed its acoustic test) user speaks over Chirp -> echo gate -> Speaker.stop()
```

### How an action on another app works (S4, typed commands for now)

```text
find_element("new note", role button) -> walk the app's accessibility tree (batched reads, limits)
                                       -> score labels -> best match first
perform(element, Press)  -> its own press action
                         -> else select (rows, cells) -> else click its centre   (reports which)
type_text / press_keys   -> synthetic key events to the app in front (caller focuses it first)
run_os_script            -> osascript with a 20 s timeout (macOS asks permission once per app)
```

Prefer a menu item pressed through the tree over a keyboard shortcut: it does not depend on key delivery (D32).

### How a spoken turn works

```text
key down -> mic on -> 16 ms frames -> local VAD (timestamps) ---------+
                                   -> Sarvam WebSocket (opened at key down, audio queued until ready)
key up   -> mic off -> flush -> transcript -> Done (menu shows it) or Error (spoken reason)
```

State changes: Listening while the key is held, Thinking from release until the transcript arrives, then Done (about a second) or Error, then Idle. A generation counter stops a slow result from an older press from overwriting a newer turn.

### Measured so far (generated clips, not real speech)

| Clip | Transcript | Flush to transcript |
|---|---|---|
| English, 2.1 s | "Open notes and write my shopping list." | 375 ms |
| Hindi, 2.8 s | "Notes खोलो और मेरी खरीदारी की सूची लिखो।" | 667 ms |
| Mixed, 2.7 s (language fixed to hi-IN) | "Notes खोलो और milk, eggs, bread लिखो।" | 423 ms |

First real-voice results (one user, three presses): one exact, one with the name misheard ("chair"), one unclear ("sharp" for "Chirp"). Flush to transcript was 1228, 646 and 618 ms.

Spoken output, measured on this Mac:

| Measure | Result |
|---|---|
| Cached line, time to start | 2 to 12 ms |
| Uncached line, request to first audio | 796 to 1026 ms (connect 357 to 555 ms) |
| Stop after `stop()`, audio callback silent | 1 to 9 ms, median 4 ms (5 trials) |

Accessibility layer, measured on this Mac (single runs, three repeats each, includes process start):

| Operation | Result |
|---|---|
| Tree walk, depth 6 | TextEdit 286 elements in 110 ms; Notes 385 in 310 to 390 ms; Finder 552 in 380 to 580 ms |
| Find by label (depth 14) | 215 to 900 ms |
| Press a button or menu item | 100 to 600 ms including the find |
| Type 40 characters (English and Hindi) | about 175 ms |
| Bring an app to the front | 90 to 310 ms when it is running |

S4 test run (`chirp --ax selftest`, last full run): TextEdit new document, English and Hindi typed and read back, Bold checkbox found and pressed; Finder new window, sidebar "Documents" and "Downloads" pressed (by clicking; they have no press action), window closed; Notes "New Note" found and pressed, text typed into the empty note and read back, test note deleted through scripting. Two earlier full runs failed because another app (WhatsApp) took the front, and Notes once failed to come to the front; with a second focus attempt it passed, but the cause of that one is not understood.

Connecting takes 340 to 690 ms but overlaps with the user speaking. Server processing is 80 to 140 ms. These are single runs on synthetic voices, so they are indicative only. The brief's target is under 3 s from end of speech to first action on the LLM path, and the STT share so far is well under 1 s.

## 6. Decisions and changes so far

Full list with alternatives in `DECISIONS.md`. Summary:

- 21 Oct 2026: project documentation is kept as Markdown files in the repo (`RESEARCH.md`, this file), not Claude Docs artifacts.
- 21 Oct 2026: viability research concluded the build is worthwhile as a prototype. Interview users early, and decide early whether Windows and NVDA should come alongside macOS. See `RESEARCH.md`.

- 21 Oct 2026: overlay window behaviour goes through `objc2` from the start, because Tauri's all-workspaces option has a reported bug (D1).
- 21 Oct 2026: overlay is hidden while Idle so idle CPU stays near zero (D2).
- 21 Oct 2026: development providers will be Sarvam (speech) and Groq (LLM) on free tiers (D6).

- 21 Oct 2026: S4 decisions D30 to D35: direct accessibility calls with batched reads, press fallback ladder, modifier keys as real events (Finder ignored flags-only shortcuts), passwords never read or typed into, per-app Automation permission, and the self-test's safety checks. See `DECISIONS.md`.
- 21 Oct 2026: S5 decisions D36 to D41: subtask planner loop, OpenAI-first model routing with Sarvam failover, rule-based fast path, first safety gate, narration policy, questions and interruption. See `DECISIONS.md`.
- 21 Oct 2026: S6 decisions D42 to D45: two opinions of risk, the spoken yes, the action log, and conversation (memory, chat replies, varied acknowledgements). See `DECISIONS.md`.
- 21 Oct 2026: D46 and D47 fix the first real-use failures (looping after the work was done, emoji, shell commands through `keys`, terminal use). D48 adds recorded voice lines for fixed phrases.
- 21 Oct 2026: D49 (sassy personality with limits, the regression it caused and its guard, spoken-text cleaning) and D50 (research on making the voice sound like the teammate).

## 7. Changelog

| Date | Change |
|---|---|
| 21 Oct 2026 | Created this file from the PRD, build brief and research. |
| 21 Oct 2026 | Scaffolded Tauri 2 app, built S0 (tray, overlay, state machine, mascot states). |
| 21 Oct 2026 | Replaced Tauri default icons with Chirp's app icon and a menu-bar template icon; product name set to "Chirp". |
| 21 Oct 2026 | Added per-state mascot animation and an entry pop; overlay window enlarged to 140 px. S0 accepted by the user. |
| 21 Oct 2026 | Built S1: permissions flow, key picker, saved config, global hold-to-talk. Accepted by the user after manual testing. |
| 21 Oct 2026 | Built S2: mic capture, local VAD, Sarvam streaming speech-to-text with stage timings, spend ledger and credit guards, and a `--transcribe-file` test command. Verified with three generated clips. |
| 21 Oct 2026 | After the first real-voice test: added keyterms for the name "Chirp" and a cautious name cleaner for transcripts. |
| 21 Oct 2026 | Built S3: audio output, synthesised sound cues, Sarvam text-to-speech with a disk cache and system-voice fallback, talk-key interrupt. The speak-over-Chirp interrupt failed its real acoustic test twice (loudness-based, then single-delay echo subtraction), so it stays off by default; the talk key is the dependable interrupt. |
| 21 Oct 2026 | Built S4: `PlatformAdapter` trait, macOS accessibility adapter (tree, find by label, press with fallbacks, type, shortcuts, focus, AppleScript), `UiNode::outline`, `chirp --ax` typed commands and `--ax selftest`. Run against real TextEdit, Finder and Notes: 15 passed, 1 skipped (Finder scripting not allowed by macOS). |
| 21 Oct 2026 | Built S5: model client with retry and failover, intent and step schemas, planner loop that breaks a request into subtasks, rule-based fast path, narrator, first safety gate, `--plan` and `--run` test commands. Spoken requests now run through the agent; the read-back is off by default. |
| 21 Oct 2026 | Built S6: rule-based risk classifier, spoken confirmation before irreversible steps (anything but a clear yes is a no), persistent action log readable aloud, and conversation features: short memory, one-call chat replies, varied acknowledgements, warmer prompts. |
| 21 Oct 2026 | Fixed the first real-use loops (do only what was asked, read typed text back, repeat detection, closing needs a yes, type any character, no terminal). Added recorded voice lines: `--voice-lines list/import/status` and playback of recordings for fixed phrases. |
| 21 Oct 2026 | Installed the teammate's first three voice lines (setup greeting, talk-key suggestion, "You are all set."), aligned the wording of those lines with the recordings, and fixed the phrase list naming the wrong suggested key. 3 of 50 lines recorded. |
| 21 Oct 2026 | Added a sassy personality (`CHIRP_PERSONALITY`), more acknowledgement lines, spoken-text cleaning (emoji broke the speech service), narration de-duplication and a finish hint after verified typing. Added `pnpm chirp` for running the test commands from the project root and generated `voice-lines-script.txt`. |
| 21 Oct 2026 | Added `--voice-lines script` (a plain-language recording script with a tone tag per line) and import by line number; generated `scripts.txt` for the person recording. |
