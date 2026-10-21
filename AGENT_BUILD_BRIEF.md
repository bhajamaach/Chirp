# Agent Build Brief: Chirp v0.1 (macOS)

*Give this whole document to your coding agent. It is the single source of truth for building v0.1.*

## 1. Mission

Build v0.1 of **Chirp**, a voice-first assistant for macOS that lets blind and low-vision people operate their computer by speaking in Hindi, English or a mix. A user presses and holds their chosen key, says what they want, and Chirp does it, narrates what it is doing, and asks before anything risky. A small always-on-top overlay near the cursor shows its state.

v0.1 is done when the **Definition of Done** in section 10 passes. Nothing more is required.

## 2. How to work

- **Build over plan.** Write a one-page plan, then start building. Do not write long design documents.
- Work in the small vertical slices in section 7, in order. Finish and test each slice before starting the next.
- Commit after every working slice with a clear message.
- Before relying on any crate, API or OS behaviour, check its current documentation. Do not assume from memory.
- When something fails, say so plainly. Never fake a result, a test pass or a measurement.
- Record every non-obvious decision in `DECISIONS.md` (what, why, alternatives).
- Ask the human only when blocked. Otherwise choose the simplest option that meets this brief and note it.
- Never commit secrets. Read keys from environment variables or a git-ignored `.env`.

## 3. Product context (it shapes every decision)

- Users may be blind and already run a screen reader. Chirp must **coexist** with it: never talk over it, and when a task fails, say so and hand control back.
- Everything must work **without looking at the screen**. Every state has a sound and a spoken cue. The overlay is a bonus for low-vision users.
- Speech is **Hindi, English and mixed Hindi-English**. Do not assume English-only anywhere.
- Safety first: irreversible actions need an explicit spoken yes. Wrong clicks cost trust.
- Tone: playful, warm and short. Spoken lines are 12 words or fewer. Never jokey in errors or confirmations. The mascot is a minimal lime squircle with dot eyes and one antenna dot; its moods (idle, listening, thinking, done) are drawn with dots and shapes, never text.

## 4. v0.1 scope

**In**

- macOS only, Apple Silicon first.
- Menu-bar app plus always-on-top, transparent, click-through overlay.
- First-run setup: permissions flow and a **key picker** ("press the key you want to use").
- Press-and-hold to talk; streaming speech-to-text; streaming text-to-speech; sound cues; "stop" interrupts.
- Execution layers 0 and 1 only: OS scripting and the macOS accessibility tree.
- LLM intent parsing and a short planner loop; a no-LLM fast path for simple commands.
- Risk classification, spoken confirmation, action log readable aloud.
- A small eval harness.

**Out (do not build)**

- Windows, Linux, browser automation, vision fallback, wake word, local models, accounts, sync, analytics.
- Keep the interfaces ready for them (section 5), but write no implementation.

## 5. Fixed decisions

- **Shell and UI:** Tauri 2. Minimal front end (plain TypeScript or a tiny framework). The overlay is a transparent, always-on-top window that ignores cursor events and is visible on all spaces and over full-screen apps. If Tauri's window APIs are not enough for full-screen and all-spaces behaviour, drop to native macOS calls through `objc2` and note it in `DECISIONS.md`.
- **Core:** Rust. Single async runtime. Modules:

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

- **PlatformAdapter trait** (macOS implements it now; Windows later): `frontmost_app()`, `get_ui_tree(depth)`, `find_element(query)`, `perform(element, action)`, `type_text(text)`, `press_keys(keys)`, `focus_app(name)`, `run_os_script(script)`, `screenshot()` (unimplemented stub in v0.1).
- **Providers are config, never hard-coded.** One trait each for speech-to-text (streaming over WebSocket, must handle Hindi-English code-mixed speech), text-to-speech (streaming, Hindi and English voices) and LLM (an OpenAI-compatible chat endpoint with JSON-schema output, so any host works). Select by environment variables. Add retry with backoff and optional failover between LLM providers.
- **Voice activity detection** runs locally.
- **State machine:** Idle, Listening, Thinking, Acting, Confirming, Done or Error. The overlay and the sound cues are driven only by this state.
- **Intent schema** (the LLM must return exactly this JSON):

```json
{
  "goal": "string",
  "app": "string or null",
  "params": {},
  "risk": "safe | reversible | irreversible",
  "needs_clarification": false,
  "question": "string or null"
}
```

Invalid JSON triggers one retry, then a spoken fallback.

## 6. Latency (measure everything)

Timestamp every stage: key down, end of speech, STT final, intent ready, first action, first spoken word. Log them. Proposed targets: end of speech to first spoken acknowledgement under 700 ms; to first action under 1.5 s on the fast path and under 3 s on the LLM path. Report the measured numbers honestly, even if they miss.

Speed rules: keep connections warm, stream every stage, start speaking on the first tokens, pre-fetch the accessibility tree while the user is still speaking, and handle simple commands (open app, scroll, back, stop) with rules and no LLM.

## 7. Build order and acceptance tests

| Slice | Build | Accepted when |
|---|---|---|
| S0 | Tauri skeleton, menu-bar icon, overlay window | Overlay stays above a full-screen app and on every space; clicks pass through |
| S1 | Key picker, global press-and-hold capture, saved config, spoken permissions flow | User picks a key at first run; holding it flips state to Listening; each permission is explained aloud |
| S2 | Audio capture, VAD, streaming STT | Spoken Hindi, English and mixed phrases appear as transcripts with timestamps |
| S3 | Streaming TTS, sound cues, interrupt | Chirp speaks replies; saying "stop" cuts it off within 300 ms |
| S4 | macOS adapter: dump the accessibility tree, find by label, press, type | Works in Notes, Finder and TextEdit from typed test commands |
| S5 | Intent parsing, planner loop, fast path, narration | Spoken command runs the action and narrates it |
| S6 | Risk classifier, spoken confirm, action log | Irreversible actions never run without a spoken yes; "what did you just do?" reads the log |
| S7 | Eval harness and results | Results file produced for the three tasks below |

For S1, test whether the Fn key can be captured on the target Mac (it is special on macOS). If it cannot, the key picker must refuse it and say why.

## 8. v0.1 demo tasks (the only three that must be bulletproof)

1. "Open Notes and write my shopping list: milk, eggs, bread." (also in Hindi)
2. "Move my latest PDF from Downloads to Documents." (Finder via OS scripting, with confirmation)
3. "What app am I in, and what is on the screen?" (read the accessibility tree aloud, summarised)

## 9. Safety rules

- Irreversible means send, pay, delete, submit, overwrite. Always restate the action and wait for a spoken yes.
- Blocklist by default: payments, passwords, system settings.
- Never store screenshots or screen text on disk by default. The action log stores actions, not content.
- Development uses fake data only. Free-tier model hosts can log prompts and are rate-limited.
- Request permissions one at a time, with a spoken reason each.

## 10. Definition of Done for v0.1

- All slices S0 to S7 accepted.
- Each of the three demo tasks run 10 times in English and 10 times in Hindi, with success rate and median and worst-case latency recorded in `EVALS/RESULTS.md`.
- No irreversible action ran without confirmation in any test.
- `README.md` covers setup, required permissions and environment variables.
- `DECISIONS.md` lists every decision and deviation from this brief.
- A final report states what works, what does not, and the measured latencies.

## 11. Start here

1. Reply with your one-page plan and any blocking questions. Useful ones: which macOS version, which provider keys exist, which screen reader the first users run.
2. Then begin S0 and keep going through the slices without waiting for approval, unless blocked.
