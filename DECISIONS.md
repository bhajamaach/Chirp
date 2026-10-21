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

## D7. Real brand assets, not scaffold defaults (S0, 21 Oct 2026)

- **What:** Product name is "Chirp". The app icon is the mascot (lime squircle with tall eyes) on a dark rounded square. The menu-bar icon is a monochrome mascot silhouette with the eyes cut out, set as a macOS template image so it tints for light and dark menu bars. Sources are in `assets/` (`app-icon.svg`, `tray.svg`); the generated icon set is in `src-tauri/icons/`, with Android and iOS icons removed because the product is macOS only.
- **Why:** The scaffold shipped the Tauri logo, which cannot ship in a product.
- **Alternatives:** A coloured lime menu-bar icon (rejected: template images are the macOS convention and stay legible in both themes). A state-aware menu-bar icon is a possible later addition.

## D8. Mascot motion (S0, 21 Oct 2026)

- **What:** Each state has its own loop: idle breathes, listening bounces and blinks with dancing sound pills, thinking sways while three dots bounce in turn, acting hops with a pulsing ring, confirming tilts and nudges for attention, done does three decaying cheer hops with a twinkling sparkle, error shakes then droops. Every state change plays a springy pop. The overlay window grew from 120 to 140 px so bounces never clip.
- **Why:** Requested feedback: several states were static.
- **Accessibility:** Under the system "reduce motion" setting all animation and transitions are disabled, so the mascot is still. This is deliberate for low-vision and motion-sensitive users. Every state also has a sound cue planned, so nothing depends on motion.

## D9. Spoken setup uses macOS `say` until S3 (S1, 21 Oct 2026)

- **What:** `voice/speak.rs` shells out to `/usr/bin/say` for the permissions flow and key picker. English only.
- **Why:** The brief wants each permission explained aloud in S1, but streaming TTS and its keys arrive in S3.
- **Limits:** Not interruptible, no Hindi, and it does not coordinate with VoiceOver, so it can speak over a screen reader. Replace with the TTS provider and a screen-reader-aware path in S3.

## D10. Active event tap with a key allow-list (S1)

- **What:** Hold-to-talk uses a session-level event tap in active (not listen-only) mode, so the chosen key is swallowed while held and never types or triggers shortcuts. Modifier keys are passed through, since dropping them can leave other apps with a stuck modifier. The picker only accepts right-side modifiers (Command, Option, Control, Shift), F1 to F20, and Fn or Globe. It refuses typing keys, left-side modifiers (used in shortcuts), arrow keys, Escape and Caps Lock, each with a spoken reason of 12 words or fewer.
- **Why:** The brief says "any key", but a held typing key would insert characters or break navigation, which hurts exactly the users Chirp is for. An active tap needs Accessibility as well as Input Monitoring, which the PRD already lists.
- **Alternatives:** Listen-only tap (needs only Input Monitoring, but the key still reaches apps); allow any key and warn.

## D11. Fn is accepted only when macOS leaves it free (S1)

- **What:** Fn or Globe is allowed as the talk key only when `defaults read com.apple.HIToolbox AppleFnUsageType` is `0` ("Press 🌐 key to: Do Nothing"). Otherwise the picker refuses it and says why, because macOS would also open the emoji picker, dictation or the input switcher.
- **Why:** The brief says to test Fn and have the picker refuse it if it cannot be used. On this Mac the setting is unset (the macOS default), so Fn is refused until the user changes it.
- **Status:** Not yet tested with a real Fn press.

## D12. Config location and format (S1)

- **What:** `config.json` in the app config directory (`~/Library/Application Support/com.chirp.app/`), holding the talk key code, its spoken label and a setup-complete flag. Written atomically (temp file, then rename). A missing or corrupt file falls back to defaults.
- **Why:** Small, human-readable, and a crash cannot leave a half-written file.

## D13. Dev builds are not their own app for permissions (S1)

- **What:** Under `pnpm tauri dev` the binary is not a bundled app, so macOS lists the launching terminal (for example Terminal, iTerm or VS Code) in Microphone, Accessibility and Input Monitoring, not Chirp. A bundled build asks for Chirp itself. A freshly granted Input Monitoring permission may only take effect after a restart, so setup retries the key tap once and otherwise says "Please restart Chirp to finish setup."
- **Why:** macOS ties permissions to the responsible app. The usage text for the microphone is embedded in the dev binary through `src-tauri/Info.plist`.

## D14. Overlay code moved to `core/overlay.rs` (S1)

- **What:** Window building, cursor following and `apply_state` moved out of `lib.rs` so the setup flow and key handler can drive the overlay.
- **Why:** `lib.rs` was becoming the place everything lived.

## D15. Sarvam's stable streaming endpoint, flushed on key release (S2, 21 Oct 2026)

- **What:** Speech-to-text uses `wss://api.sarvam.ai/speech-to-text/ws` (Saaras v4) with `flush_signal=true`. Audio is raw 16 kHz 16-bit PCM, base64 in JSON, in batches of about 96 ms. On key release the client sends `{"type":"flush"}`, which finalises the transcript without waiting for the server to detect silence. The connection opens on key down and overlaps with the user speaking.
- **Why:** The brief asks for streaming STT over WebSocket. The newer `speech-to-text-realtime` endpoint (beta) returns partial transcripts, but the stable one is enough for press-and-hold and is not beta. The message format was read from Sarvam's official Python SDK because the reference pages omit it.
- **Result:** The first live test accepted raw PCM with `encoding: "audio/wav"` and `input_audio_codec=pcm_s16le`. A WAV-wrapped fallback exists (`STT_CODEC=wav`) but was not needed.
- **Alternatives:** The realtime beta endpoint (revisit if partials matter for latency); the REST endpoint (not streaming).

## D16. Language fixed to Hindi with code-mixed output (S2)

- **What:** Default `STT_LANGUAGE=hi-IN` with `STT_MODE=codemix`.
- **Why:** With auto-detect (`unknown`), a Hindi-English test clip was labelled English and came back as romanised Hindi with a missing space ("Notes KholoAur Milk Eggs Bread LIKHO."). With `hi-IN` the same clip came back correct ("Notes खोलो और milk, eggs, bread लिखो।"), and a pure English clip stayed exact ("Open notes and write my shopping list.").
- **Caveat:** The evidence is three clips made with macOS's synthetic voices (Samantha, Lekha), which is not real speech. It needs checking with real voices, accents and noise before S7. Revisit if English-only speech degrades.

## D21. Keyterms bias recognition toward "Chirp" (S2, 21 Oct 2026)

- **What:** The Sarvam connection sends `keyterms=["Chirp"]` by default (`STT_KEYTERMS`, comma separated, empty turns it off). Saaras v4 supports up to 50 terms of 64 characters. It is skipped on other models.
- **Why:** A first real-voice test heard "Hey chair how are you doing man" for what was presumably "Hey Chirp...". A product name is outside any recogniser's vocabulary, and keyterms are Sarvam's tool for that.
- **Evidence:** The server accepts the parameter. A synthetic clip was transcribed correctly with and without it, so the benefit is unproven. It needs checking on real voices. Add app and file names ("Notes", "Finder", "TextEdit") if they are misheard.
- **First real-voice results (one user, three presses):** one exact, one misheard the name, one unclear. Flush to transcript was 1228, 646 and 618 ms. The first press also had a 545 ms microphone start-up against about 170 ms afterwards, and a buffer-underrun glitch from the audio device, which the stream recovered from. Those glitches are now logged as non-fatal.

## D22. One output stream, one queue, for speech and sound cues (S3, 21 Oct 2026)

- **What:** `voice/playback.rs` owns a single cpal output stream fed by one queue. Spoken replies and sound cues both go through it, so stopping is one call. The stream is paused 1.5 s after the queue empties, so an idle Chirp holds no audio hardware. Mono audio is upsampled to the device rate with streaming linear interpolation.
- **Measured:** after `stop()`, the output callback goes silent in 1 to 9 ms (median 4 ms over 5 trials, `chirp --stop-test`). The target is under 300 ms. This is measured to the point where our callback outputs silence; the device's own buffer adds roughly 10 to 20 ms more, which was not measured with a loopback.
- **Alternatives:** a separate stream per sound (stopping then needs many handles); rodio (extra dependency, less control over the interrupt).

## D23. Sound cues are synthesised, one per state (S3)

- **What:** Listening is a short rising tone, Thinking one soft tick, Done two rising notes, Error a falling tone. Idle is silent, and Acting and Confirming are spoken instead. Cues are generated in code with fades so they cannot click, and play from a thread subscribed to state changes (each one is logged as `[chirp] cue for <state>`). `CHIRP_CUES=0` turns them off.
- **Volume (changed after the first real test):** the first level, a 0.22 peak, was too quiet: the user heard nothing, and `chirp --cue-test` measured the cues at only 1.3 to 2.7 times the room level on the MacBook speakers. They are now 0.5 (tick 0.4), measured at 2.7 to 5.1 times the room level. The error tone sat at 190 to 330 Hz, which laptop speakers barely reproduce, so it now sweeps 560 to 360 Hz. It is still the weakest at about 1.7 times, which is acceptable because every error is also spoken.
- **Why:** The PRD's state table pairs each state with a sound, and nothing may depend on sight. Generated tones need no asset files and cost no credit.
- **Limits:** The Listening cue plays as the microphone opens, so it can bleed into the start of the recording. Thinking is one tick, not a repeating one. Both are untested by ear on real use and the tones are placeholders until sound design is confirmed (PRD open question).

## D29. Reading back what was heard, in the free system voice (S3)

- **What:** After a transcript, Chirp says "You said: <command>" in the macOS voice (Lekha for Devanagari text, the default voice otherwise). Off with `CHIRP_READBACK=off`.
- **Why:** Until Chirp can act on a command (S4 and S5), a spoken command gets no audible reply, which looked like "not responding". The read-back lets the whole chain, hearing then understanding, be heard working. The system voice costs nothing, and the text contains the user's words, so it must never go to the paid voice or the cache.
- **Temporary:** S5 replaces it with real narration. Whether a read-back stays as a verbosity option is open (the PRD has a terse and detailed setting).

## D24. Bulbul v3 for speech, with a disk cache and a system-voice fallback (S3)

- **What:** Spoken output uses Sarvam `bulbul:v3` (speaker `ritu`, 24 kHz WAV) over `wss://api.sarvam.ai/text-to-speech/ws`, sending `config`, `text` then `flush` per line, with `send_completion_event=true`. Hindi is chosen when a line contains Devanagari, otherwise Indian English. Lines are tried in this order: a cached recording, then Sarvam, then the macOS voice, so Chirp is never silent when the network, key or credit is missing. Fixed app phrases are cached under `~/Library/Caches/com.chirp.app/` and cost credit once; anything containing the user's words must use `say_uncached` and is never stored.
- **Why v3:** Its price is published (Rs 30 per 10,000 characters). v4-flash is the recommended newer model but is not on the price list, and the credit is Rs 100. Switch with `TTS_MODEL` and a persona `TTS_SPEAKER` if its price turns out to be the same.
- **Measured:** connect 357 to 555 ms, first audio 796 to 1026 ms after the request (so about 0.4 to 0.5 s of generation after connecting), and a cached line starts in 2 to 12 ms. A fresh connection is opened per line, so the first word of an uncached line pays the connect time; warm connections are a later optimisation.
- **Shared budget:** the credit ledger now counts characters as well as seconds of listening against one Rs 90 cap, and a line that would pass it falls back to the system voice.

## D25. The talk key always interrupts Chirp (S3)

- **What:** Pressing the talk key stops whatever Chirp is saying, drops any lines queued behind it, and then starts listening. This needs no microphone, no detection and no credit, so it is the dependable interrupt.

## D26. Speaking over Chirp: echo cancellation, off by default (S3)

- **What:** While Chirp speaks, an optional watcher opens the microphone and decides whether a sound is the user or Chirp's own voice coming back. It keeps a 16 kHz copy of what was sent to the speakers, finds the echo delay (up to 150 ms) by correlation, subtracts the best-fitting echo from each 16 ms microphone frame, and looks at the leftover. Pure echo cancels almost completely; a voice does not. A voice counts when the voice detector fires and the leftover exceeds three times the usual leftover for 4 frames (64 ms). The first 12 frames after the microphone opens are ignored, because a fresh stream glitches.
- **Why not loudness:** A first version compared the microphone level with the output level. In a real test on this Mac's speakers and microphone it interrupted itself once in four plays (the echo estimate came from 14 noisy frames and varied from 0.14 to 0.30 for the same room) and missed an outside "stop" once in three, because the outside voice was only about 0.05 louder than the echo.
- **Result of the real acoustic test: it FAILED.** 13 synthetic tests pass, but in the real room (`chirp --barge-test`) the gate interrupted Chirp itself in 2 of 4 solo plays. Part B's "3 of 3" is not valid: two trials stopped before the outside voice began (reported as 0 ms), so those were false triggers too. The measured outside-voice delay, 343 ms in the one trial that may be genuine, also misses the 300 ms target.
- **Why (from the recorded frames):** the echo is not a clean delayed copy of the output. The delay the search found wandered between 55 and 150 ms from frame to frame, because speech repeats at every pitch period and the room and speaker add reverb and processing. Subtracting one delayed copy removed only about 40% of the echo, so a large leftover remained, and the calibrated leftover swung from 0.2 to 1.3 between runs. When no delay matched well, the gate treated the whole microphone as the user, which produced the self-interruptions.
- **What would fix it:** a real adaptive echo canceller (a multi-tap NLMS filter, trained on recorded microphone and output audio so it can be tuned silently offline), or macOS's own voice processing (the audio unit FaceTime uses), which would mean routing both capture and playback through it. Either is a larger piece of work than this slice.
- **Not yet tested:** with headphones there is no echo path, so the gate should behave as a plain voice detector and work. That is untested here.
- **Stays off by default, and is experimental:** a false trigger cuts a reply off mid-sentence, which is worse than a miss, because the talk key already interrupts. Enable with `CHIRP_BARGE_IN=1`, ideally with headphones. `chirp --barge-test` runs the acoustic test (audible, about 25 seconds), and `CHIRP_BARGE_DUMP=<path>` records every frame the gate sees.
- **Consequence for the brief:** S3 asks that saying "stop" cut Chirp off within 300 ms. That is **not met on laptop speakers**. It is met by pressing the talk key (1 to 9 ms), and unverified for the voice with headphones.
- **Limits:** A voice only counts as an interrupt, not as a command; to give the next instruction the user still holds the talk key. The microphone opens whenever Chirp speaks while this is on.

## D27. Dev builds are lightly optimised (S3)

- **What:** `[profile.dev] opt-level = 1` in `Cargo.toml`.
- **Why:** The echo search runs in real time, and an unoptimised build could not always finish a frame in 16 ms. It also cut the test suite from 8 s to under 1 s.

## D28. The credit ledger re-reads its file before every change (S3)

- **What:** `Ledger` refreshes from `usage.json` before adding spend or checking the budget.
- **Why:** The running app and a test command each held their own in-memory totals, and the app's later save overwrote 110 characters of speech spent by the interrupt test. The estimate under-counted by about Rs 0.33, the unsafe direction for a Rs 100 balance. I corrected that day's file by hand (118 characters, 31.4 seconds, about Rs 0.62 in total) and added a test with two copies sharing one file.
- **Limit:** Two copies saving within the same instant could still race. That window is tiny and the estimate errs, if at all, by one line.

## D17. Credit guards for a Rs 100 balance (S2)

- **What:** A spend ledger (`usage.json`, seconds only, never audio or text) estimates cost at Rs 30 per hour of audio sent. Speech calls stop at an estimated Rs 90 (`SARVAM_BUDGET_RUPEES`). One recording is capped at 20 seconds (`CHIRP_MAX_UTTERANCE_SECS`). Presses under 300 ms never reach the API. Audio is never re-sent on failure. The test command refuses clips over 15 seconds.
- **Why:** The only credit is the Rs 100 signup balance, shared across all Sarvam APIs. The estimate is not the real balance; the Sarvam dashboard is.
- **Measured so far:** five test clips of 2 to 3 seconds each came to about Rs 0.10 in total.

## D18. Microphone only runs while the key is held (S2)

- **What:** The capture stream is created on key down and dropped on key up. Audio is mixed to mono, resampled to 16 kHz with a box filter (any input rate), and cut into 16 ms frames.
- **Why:** Privacy and trust: no recording indicator and no audio between turns. The cost is the microphone start-up delay on every press, which is logged as "mic ready" so it can be measured.
- **Alternatives:** Keep the stream open and discard audio when idle (lower start-up delay, but the mic is always on).

## D19. Local voice detection is diagnostic, not a gate (S2)

- **What:** Earshot scores every 16 ms frame, and a tracker (64 ms of voice to start, 400 ms of quiet to end) records when speech started and ended. These feed the timing log. Audio is sent to the provider whenever the key was held for at least 300 ms, whatever the detector heard.
- **Why:** A detector that misses soft speech would silently drop a valid command. Press-and-hold already marks the end of speech. The detector becomes a gate or a hands-free trigger later if needed.
- **Choice of detector:** Earshot (pure Rust, about 95 KiB, 16 kHz, MIT or Apache) over Silero or TEN, which need ONNX Runtime.

## D20. Transcripts live in memory and the terminal only (S2)

- **What:** The last transcript shows in the menu (dev aid) and in the terminal log. Nothing is written to disk. Only the spend ledger and the key config are persisted.
- **Why:** The brief says not to store screen text by default, and a transcript is the user's speech.

## D30. The accessibility API is called directly, with one batched read per element (S4, 21 Oct 2026)

- **What:** `platform/macos/ax.rs` declares the handful of `ApplicationServices` functions it needs and wraps them (`AxElement`, `AxValue`) over `core-foundation`. Each element is read with one `AXUIElementCopyMultipleAttributeValues` call for 13 attributes, not 13 calls. `AXUIElementSetMessagingTimeout` is set to 2 s so a hung app cannot block for the system default of 6 s.
- **Why:** The surface is small and no extra crate is needed. Batching matters for speed, because every attribute read is a round trip to the other app.
- **Measured:** a depth-6 walk takes 110 ms for TextEdit (286 elements), 310 to 390 ms for Notes (385) and 380 to 580 ms for Finder (552), about 0.4 to 1 ms per element including process start. That is fast enough to pre-fetch the tree while the user is still speaking, as the brief asks.
- **Limits per walk:** 3000 elements, 4 s, and a depth chosen by the caller (14 for `find_element`). A node cut off by a limit is marked `truncated`. Text values are cut to 200 characters, so a long document is never copied whole.
- **Alternatives:** the `accessibility` / `accessibility-sys` crates (more code than needed here, and they read attributes one at a time).

## D31. `press` falls back to select, then click, and refuses disabled controls (S4)

- **What:** `perform(Press)` tries the element's own press action. If the element truly has none (error "action unsupported", "attribute unsupported" or "failure"), a row or cell is selected, and anything else is clicked at its centre. The result (`Directly`, `Selected` or `Clicked`) goes back to the caller for the action log. If the app is merely busy, or the element is invalid, there is no fallback. A disabled control is refused with an error, because macOS accepts the press and does nothing, which would be reported as success.
- **Why:** Finder's sidebar entries are cells with no press action; in the test they needed the click fallback. A click moves the pointer, which a screen-reader user may not expect, hence the explicit report of how it was done.
- **Not yet done:** selecting the cell's parent row (cleaner than clicking), and checking that the clicked point is not covered by another window. The click uses the frame from the same walk, so a window moving between find and press could misdirect it. S5 should re-find just before pressing.

## D32. Shortcuts press the modifier keys as real events (S4)

- **What:** `press_keys("cmd+n")` posts a Command key-down, then the key with the Command flag set, then the Command key-up. An earlier version only set the flag on the main key.
- **Why:** Flags-only worked in TextEdit and Notes, but Finder silently ignored it: ten tries of Command+N and Command+W changed nothing, while System Events' keystroke worked at the same moment. Posting the modifiers as keys fixed it.
- **Guidance for S5:** where an app has a menu item for the action, pressing the menu item through the tree (`press "New Finder Window" --role menuitem`) does not depend on keyboard delivery at all, and worked every time. Prefer it to shortcuts. Shortcut key codes assume a US layout.

## D33. Passwords are never read or typed into (S4)

- **What:** For elements with subrole `AXSecureTextField`, the value is never read, so it cannot reach a log, the terminal or a prompt. `type_text` refuses when the focused element is a secure field. Both follow the brief's blocklist for passwords.
- **Not tested:** no password field was used in the tests (fake data only), so this is covered by code reading, not by a run.
- **Privacy note for S5:** the tree shows real on-screen text (the Notes test printed a line of an existing note in the terminal). Anything sent to the LLM must be the short outline, and its limits (200 characters per value) are the only protection so far. The action log must store actions, not these values.

## D34. Scripting needs a macOS permission per app, and it blocks (S4)

- **What:** Running AppleScript against another app makes macOS show "wants access to control <app>" the first time, and the script waits for the answer. Chirp's script runner times out after 20 s and says an Automation prompt may be waiting. The error "Not authorised to send Apple events" (-1743) becomes `AutomationDenied(<app>)` so it can be spoken.
- **Seen in the test:** TextEdit and Notes were allowed (for VS Code, which launches the dev build); Finder returned -1743 at once and never showed a prompt, so its scripted check is skipped, not passed. While a prompt is up it holds the front and swallows keystrokes, so the app tests fail until it is answered. The adapter's tree can read such a dialog (its text and buttons), which is how it was found.
- **Open:** the spoken permission flow (S1) does not cover Automation. Because the prompt only appears on first use and is per app, the first scripted action on each app will pause. Deciding whether to trigger the prompts up front in setup, or to avoid scripting where the tree can do the job, is left for S5 and S6.

## D35. How the S4 test protects the machine it runs on (S4)

- **What:** `--ax selftest` uses fake text, checks that the right app is in front before every key or character it sends (and re-focuses once if something took the front), types into a Notes note only after it sees an empty focused note, and deletes only a note whose title starts with "Chirp S4 test". TextEdit's document is closed with "saving no", and Finder's window is closed again.
- **Why:** The test drives the user's real apps, and a chat app took the front in the middle of two runs. Without the check the Hindi text would have been typed into WhatsApp.
- **Leftover:** deleted notes stay in Notes' Recently Deleted for 30 days. My early manual tests also left a few extra Finder windows open; they are harmless.

## D36. A planner loop that breaks a request into subtasks and does them one action at a time (S5, 21 Oct 2026)

- **What:** Two kinds of model call. First `Intent` (the brief's schema): goal, app, params, risk, needs_clarification, question. Then a loop of `Step` calls. On its first turn the model writes a `plan` of 2 to 6 plain-word subtasks; every turn it also names the subtask it is on and picks exactly one action (`focus_app`, `press`, `type`, `keys`, `script`, `wait`, `done`, `ask`, `fail`). Chirp checks the action, runs it, waits 250 ms, reads the front app's outline again and asks for the next step. The model can rewrite the plan when something fails. The loop ends on `done`, `ask` or `fail`, after 12 actions, after 3 failures in a row, or when the same action fails 3 times (it says it is going in circles and hands control back).
- **Why one action per turn:** the screen changes after every action, so a long plan written up front goes stale. The cost is one model call per action.
- **Deviation from the brief:** `params` is not an open object. A strict JSON schema cannot describe free-form keys, so it has four named nullable fields (`text`, `target`, `location`, `query`). The rest of the intent schema is as written.
- **Invalid replies:** every reply is validated again in Rust. An invalid one gets one retry that quotes the problem; a second failure ends the task with a spoken "I didn't understand how to do that".
- **The loop is generic** over `PlatformAdapter`, `LlmClient` and `Narrate`, so 15 tests run whole tasks against a fake Mac and a scripted model: a three-subtask Notes task, a bad reply, cancel, refusal, stuck detection, the step limit and so on. No real app was touched by these tests.

## D37. Language models: any OpenAI-compatible host, OpenAI first, Sarvam as failover (S5)

- **What:** `llm/` posts to `/chat/completions` with a strict `json_schema` response format. Providers are tried in the order of `LLM_PROVIDERS` (default `custom, openai, sarvam`), each only if its key is present. Network errors, 429 and 5xx are retried twice with backoff (`retry-after` honoured up to 4 s); a 400, 401 or 403 moves straight to the next provider. Hosts differ in small ways that are handled in one place: OpenAI gets `max_completion_tokens` and no temperature on gpt-5 and o-series; Sarvam gets `reasoning_effort: null`, since its model otherwise thinks and bills for it.
- **Default model:** `gpt-4.1-mini` on the user's own OpenAI key. It is fast, supports strict structured output and is cheap. The `.env` also holds Anthropic and Gemini keys; they are not used, because Anthropic's own API is not OpenAI-shaped and its compatibility endpoint does not do strict schemas. Some commented-out keys in the file are labelled as other people's, and were left alone.
- **Sarvam as failover:** `sarvam-105b` at Rs 15 per million input tokens and Rs 60 per million output. Its spend is added to the same credit ledger and the same Rs 90 cap as speech. OpenAI spend is not tracked: its balance is not the Rs 100 credit.
- **Measured, live:** intent 1.4 to 3.4 s (362 to 370 input tokens, 53 to 60 output), first planned step 1.5 to 2.2 s (780 to 810 input tokens). That puts the first action at roughly 3 to 5.5 s after the transcript, **over the brief's 3 s target on the model path**. The fast path has no model call. Possible fixes, not done: one combined intent-and-step call, a smaller model, a warm connection at start-up.
- **Privacy:** the request and a 2,500-character outline of the front app's window go to the model host on every step. The outline holds real on-screen text. `--plan` leaves the screen out unless `--screen` is passed. Password field values are never read (D33).

## D38. A rule-based fast path for simple commands (S5)

- **What:** `core/fastpath.rs` handles "open <app>" (English and Hindi, with Hindi app names mapped, for example नोट्स to Notes), "scroll up or down", "go back", "stop" and "what app am I in" with no model call. A request with a second clause ("open Notes and write ...") is never taken, so it goes to the planner.
- **Why:** the brief asks for it, it needs no key and no credit, and it starts at once.
- **Limits:** "open <anything up to three words>" tries that app name, so "open the door" ends in "I couldn't find that app". Scroll sends Page Down and Page Up, and back sends Command+[, which not every app understands.

## D39. A first safety gate, before the real one (S5; superseded by D42 and D43 in S6)

- **What:** `safety::check` runs before every action. A step marked irreversible never runs (there is no spoken yes until S6), a whole request marked irreversible is refused before any action, scripts containing delete, shell, send, password, keychain, settings and similar words are refused whatever the model says about their risk, payment-looking buttons are not pressed, and System Settings is not opened. A refusal ends the task with a spoken reason and returns control.
- **Why:** S5 lets a model choose actions, so the blocklist and the irreversible rule from the brief cannot wait for S6. S6 adds the spoken confirmation that will let an irreversible step through, the classifier, and the persistent log.
- **Limits:** word lists are a blunt instrument. They catch the obvious cases and will refuse some harmless scripts (a script that merely mentions "delete" in a string). The model's own risk label is trusted for everything not on a list, which is exactly what S6 has to test.

## D40. Narration: a short acknowledgement, up to three step lines, then the result (S5)

- **What:** "On it." is a fixed phrase (cached on disk after the first time, so it starts in a few milliseconds). The model supplies a short `say` line with each step; the first three are spoken, then the final result is always spoken. Lines go through one narrator thread, so the work never waits for speech, and pressing the talk key drops everything queued. Lines with the model's or the user's words are never cached and cost Sarvam credit (about Rs 0.003 a character) or fall back to the free system voice.
- **Why:** the brief asks for narration and a first acknowledgement quickly, and a screen-reader user needs to know what happens; but a spoken line for every click would be slow and spend credit.
- **Replaces D29:** the "You said: ..." read-back is now off unless `CHIRP_READBACK=on`.
- **Not decided:** the PRD's terse and detailed verbosity setting. The three-line cap is the only control.

## D41. Questions and interruptions (S5; the pending-question part replaced by D45)

- **What:** If a request is missing one fact, Chirp asks one short question (overlay in the Confirming look), remembers the request, and joins the user's next spoken answer to it. Pressing the talk key at any point cancels the running task before its next step, silences speech and starts listening. A new task waits for the old one to notice it was cancelled, so two tasks never act at once.
- **Limit:** a step already running (a Finder script, a 4 s wait) finishes before the cancel takes effect.

## D42. Two opinions of risk, and the higher one wins (S6, 21 Oct 2026)

- **What:** Every step has a risk from the model (`safe`, `reversible`, `irreversible`) and a second one from `safety::classify`, which reads the action itself: a press on a control whose label has a whole word such as send, delete, remove, trash, discard, submit, post, publish, replace, overwrite, reply, forward or share; a script containing delete, trash, move, send, empty, remove, overwrite, replace or rename; the shortcuts Command+Delete and Command+Return. The higher of the two is used and recorded. `safety::check` then returns Refuse, Confirm or Allow.
- **Why:** The brief says irreversible actions never run without a spoken yes. A model that calls "Send" safe once must not be able to skip the question, so the rules cannot be argued out of it. Opening apps and typing are never confirmed (typing the word "delete" is not deleting).
- **Changes S5's D39:** moving, deleting and sending used to be refused; they are now confirmable, because the brief's demo task 2 (move a PDF) is meant to run "with confirmation". What stays refused, with no question asked: shell scripts, sudo, System Settings, keychain and password scripts, restart and log-out scripts, formatting or erasing, and payment buttons (pay, purchase, buy, place order, subscribe).
- **Limits:** word lists will both miss things (a button called "Go" that sends) and over-ask ("Share" in a harmless menu). Over-asking is the safe direction. A control the model labels with an unusual word and the rules do not know is the real gap, and the S7 evals should probe it.

## D43. The spoken yes: restate, wait, and treat anything unclear as no (S6)

- **What:** Before an irreversible step Chirp says what it is about to do, naming the real action and the app ("I'll press Send in Mail. Is that okay?"), with the model's one-line reason in front when it gave one. The overlay shows the Confirming look. The user answers by holding the talk key, as for any command, and the next utterance is the answer instead of a new task. The answer is read by `safety/confirm.rs`: a yes is a short answer (six words or fewer) made only of yes-words, English or Hindi ("yes", "go ahead", "haan", "ठीक है", "कर दो"); any no-word anywhere makes it a no ("yes, wait" is a no); anything else is unclear. Unclear is asked once more ("Sorry, was that a yes or a no?"), then treated as no. Silence for 45 s is a no. A yes covers only that one step: the next irreversible step asks again. The action that runs is the very object that was described, so what was read out is what runs.
- **Pressing the talk key while waiting** means "I am answering": the question stops and the task keeps waiting. It does not cancel. After a no, the task ends with "Okay, I'll leave it." and control returns to the user.
- **Why hold the key instead of listening on its own:** the microphone stays off between turns (D18), and the echo canceller that would let Chirp listen while it speaks failed (D26). It costs one more key press per question.
- **Tested:** 5 tests of the wait loop and 5 of the parser, including mixed answers, long sentences, unrelated commands and silence; the agent tests cover yes, no, interrupt, a model that mislabels risk, and several risky steps in one task. **Not tested:** a real spoken answer through the microphone, or in a noisy room. Hindi answers are verified only as text.

## D44. The action log, and reading it aloud (S6)

- **What:** `safety/log.rs` keeps one record per action: task id, time, step number, what was done in the imperative and in the past tense, risk, whether the user confirmed it, and whether it worked. Saved as `actions.jsonl` next to the config, newest 200 kept. "What did you just do?" (and "what did you do", "read the log", "तुमने अभी क्या किया") is answered by rule, with no model, from the last task: "I opened Notes, pressed "New Note" and typed 17 characters." A step you said no to is read as "asked to press Send, and you said no", a failed one as "tried to ... but it didn't work". At most six steps are read, then "and 3 more steps". Reading the log never changes it.
- **Actions, not content:** typed text is "typed 17 characters", a script is "ran a script", and the user's request, transcripts and screen text are never written to the log. Control labels that were pressed are (they are the action).
- **Limits:** the log is in English only. It records the fast path too, except reading-only commands. It is not yet possible to ask about older tasks than the last one.

## D45. Conversation: a short memory, chat in one call, varied acknowledgements (S6)

- **What:** (1) `core/memory.rs` keeps the last six exchanges (each side cut to 160 characters), forgets them after 10 quiet minutes, and holds them in memory only. They go to both model prompts, so "do that again" and "also add bread" work, and an answer to a question Chirp asked is understood without a special pending state. (2) The intent gains one field, `reply`: for a greeting, thanks or a question that needs no action, the model answers in that one call (about 25 words, in the user's language) and no steps run. (3) The quick acknowledgement rotates through four English phrases and three Hindi ones (Hindi when the request has Devanagari), and is skipped when the request looks like conversation (a question mark, or an opening word like hello, what, how, can), since "On it" before a chat answer is odd. (4) The prompts ask for natural, warm wording, in the user's language, with `say` lines that vary; clarifying questions, results and failures are written by the model in plain, kind words with a next step where there is one. Chirp's own fixed lines stay short.
- **Deviation from the brief:** the extra `reply` field on the intent, and spoken results up to 20 words (the brief's 12-word limit stays for the narration lines). Chat answers are about 25 words.
- **Replaces D41's pending-question state:** the conversation memory does the same job more naturally.
- **Privacy:** the last six exchanges, in the user's words, go to the model host with each request. They are not written to disk.
- **Measured live:** a chat reply came back in one call, 2.2 to 8.0 s (the 8 s was an outlier on the network). A Hindi greeting got a Hindi answer. An ambiguous request ("reply to Rahul's email and send it") produced a natural clarifying question instead of a guess.
- **Not yet conversational:** Chirp still needs the talk key held for every turn, so a back-and-forth is key-press, speak, release, listen. Hands-free listening after a question is the obvious next step and depends on a working echo canceller (D26).

## D46. Fixing the first real-use loop: it kept "saving" after the work was done (S6, 21 Oct 2026)

- **What happened (the user's first real run):** "make a shopping list" opened Notes, made a new note and typed the list (steps 1 to 3, all fine), then looped for nine more steps: Cmd+S, Cmd+W, Cmd+N, Cmd+W and so on, until the 12-step limit, 33 s and about Rs 1.1 of speech. The model had put "save the note" in its own plan and followed it. Notes saves by itself, and Cmd+W closed the note, which made the text vanish from the screen and sent the model round again.
- **Fixes:** (1) The prompt now says to do only what was asked, never to add save, close or tidy-up subtasks, never to press Cmd+S or close or quit unless asked, and to call `done` as soon as the screen shows what was requested. (2) After typing, Chirp reads the focused field back and puts the answer in the step result ("the text now shows in the field" or "the field does not show it yet"), so the model is told instead of guessing from an outline. (3) Cmd+W and Cmd+Q now need a spoken yes, like other irreversible steps, since closing can lose work. (4) The same action three times in the last five steps ends the task ("I seem to be going in circles"), even when every attempt succeeds; before, only repeated failures stopped it.
- **Not confirmed on a real run:** these are checked by tests with a fake Mac and a scripted model, not by repeating the Notes request. The model may still find another way to loop; the step limit and the new repeat check are the backstop.
- **Side finding:** the speech-to-text heard "lychee" (probably) as "laughing" in the user's sentence. Chirp acted on the transcript as heard, and has no way to know.

## D47. Second real-use failures: emoji, "keys" used as a keyboard, and the terminal (S6, 21 Oct 2026)

- **What happened:** (1) "Put a pink heart next to I love Nancy": the model opened the emoji picker, typed into its search box, pressed Escape three times and stopped ("going in circles", which is the D46 check working). (2) "Make a notes.md in this repository": the model opened Terminal and tried to send a whole shell command through the `keys` action, three times.
- **Fixes:** the prompt now says `type` writes any character including emoji (type the emoji itself, never open the picker) and is the only way to write text; `keys` is one shortcut or key and never text. The "unknown key" error now says so, so a model that makes this mistake is told what to do instead of retrying blind. Opening Terminal, iTerm, Warp, Ghostty, kitty, Alacritty or Hyper is refused ("I don't use the terminal.").
- **Why the terminal refusal matters:** a terminal runs whatever is typed into it and Return runs it, which would get around the shell-script blocklist (D42). It does not cover a terminal that is already in front (the VS Code terminal, say), so a request made while one is focused could still type into it. Closing that needs a check on the front app before typing and is not done.
- **Not fixed:** Chirp cannot make a file "in this repository" because it does not know which folder a code editor has open. The honest answer is the refusal; creating a file through TextEdit and the save dialog is possible but was not built.

## D48. Recorded voice lines for fixed phrases (S6, 21 Oct 2026)

- **What:** `voice/phrases.rs` lists the 50 fixed English lines Chirp says (setup, key-picker refusals, acknowledgements, "Is that okay?" follow-ups, failures, refusals) plus 3 Hindi acknowledgements. A fixed line with a recording in `~/Library/Application Support/com.chirp.app/voice-lines/<slug>.wav` is played from that file instead of being synthesised: instant, free, and in a person's voice. Order for a fixed line: recording, then the cached synthetic line, then Sarvam, then the macOS voice.
- **Workflow:** `chirp --voice-lines list` prints the lines with the file name for each, as a recording script; `import <folder>` takes WAV, MP3, M4A, AIFF, CAF or FLAC, matches files to phrases by name (the slug or the words themselves), converts to mono 16-bit 24 kHz with `afconvert`, trims silence at both ends (keeping 40 ms), levels the peak to about -1 dBFS and installs them; `status` shows what is still missing.
- **Fixed lines are decided in one place:** the narrator treats any line that is in the list as fixed even if the caller sent it as ordinary text, so refusals and failure lines get recordings and caching without each call site changing. Tests fail if the agent's acknowledgements, failure lines or the safety refusals are missing from the list, so a new fixed line cannot be forgotten.
- **What it does not cover:** anything the model writes (results such as "I've opened Notes and written...", clarifying questions, chat answers, step narration) and lines with a name in them ("Opened Notes.") are still the Sarvam voice, so a conversation will switch between voices. Options for later: narrate common actions from fixed templates that can be recorded, or a voice-cloning speech provider.
- **Rights:** the recordings are the user's teammate's voice and the user said they have the right to use it. No cloning is involved in this step: the clips are played as recorded.
- **First recordings (21 Oct 2026):** the teammate's first three clips were identified with speech-to-text (about Rs 0.20) and installed. Their wording differed from the script, so Chirp's lines were changed to match the recordings: "Hi, I am Chirp. Let's set up a few permissions." (was "Welcome to Chirp..."), "You are all set." (was "All set."), and "Press your talk key. I suggest the right option key." The last one also exposed a bug in the phrase list, which named Right Command as the suggested key while setup suggests Right Option; it now reads the key from setup. A fourth clip ("Chirp brown fox jumps over the big fence") is a test, not a line, and was not installed. Playback from a recording was checked once: 4 ms to start, no credit used. 3 of 50 lines are recorded.
- **Handing it to the recorder (21 Oct 2026):** `--voice-lines script` writes `scripts.txt`: how to record, a tone tag per line (cheeky, friendly or calm, with errors, refusals and yes/no follow-ups always calm) and numbered lines, with the recorded ones marked to skip. Files can be named by number ("14.m4a"), because long hyphenated names are a poor thing to ask a person to type; import prints which phrase each file matched. The cost of numbers is that they depend on the order of the list, so new lines are added at the end and the script is regenerated after any change.
- **Tested:** slugs, the unique file name per phrase, silence trimming and levelling, loading, refusing over-long clips, and a real import of tone files in three formats and rates through `afconvert`. **Not tested:** playing a real recording through the speakers inside the app.

## D49. A sassy personality, with limits, and what broke when it was added (S6, 21 Oct 2026)

- **What:** `CHIRP_PERSONALITY` is `sassy` (the default), `warm` or `plain`. The sassy prompt describes a quick-witted sidekick: playful teasing, dry one-liners, short punchy lines, casual Hinglish in Hindi, no corporate phrases ("How can I assist you", "certainly", "as an AI"). Teasing is about the situation, never about the user's abilities, accent, mistakes or disability. Sass stops completely, and Chirp is plain and exact, when it asks a yes or no, refuses, reports a failure or the user sounds stressed (the brief: never jokey in errors or confirmations). The confirmation question and every fixed error and refusal line stay plain by construction, not by the model's choice. The narration lines and the final message carry the personality. The acknowledgement pool grew from 4 to 7 English lines ("Say less.", "Consider it done.", "You got it, boss."...) and Hindi from 3 to 4.
- **Checked live (model only):** "how are you" gave "Fabulous. What are we breaking today?", "I'm proud of you" gave "Flattery will get you everywhere.", and task narration became "Notes, coming right up." and "Safari, let's hunt some pizza."
- **A regression I caused, and its guard:** my first version put narration examples ("Notes, coming right up.") in the prompt that reads the request. The model then answered "open Notes and write hello" with that quip in `reply`, which makes Chirp talk instead of act. The examples now live only in the step prompt, the intent prompt says `reply` is always null for anything done on the Mac, and a test fails if narration examples reach the intent prompt. A sassy tone near an action choice is a real risk, so S7's evals should include plain commands with the sassy persona on.
- **Spoken text is cleaned:** a reply that ended in an emoji made Sarvam's speech service fail ("Text must contain at least one character from the allowed languages"). Text is now stripped of emoji, symbols and markdown marks before it is spoken, and the prompts say to write none. Typing an emoji into a document still works and is unaffected.
- **Also fixed from the same session:** the model announced "I added the pink heart" in a narration line and then kept acting instead of finishing, so the typing result now ends "If that was everything asked, the next action is done" and the prompt says that saying something is done does not finish the task; and the same narration line is never spoken twice in a row.
- **Open:** the recorded lines are neutral. A sassy voice needs the teammate to record the sassy acknowledgements and to deliver the lines with the attitude, since a recording keeps whatever delivery it was given.

## D50. Making the synthetic voice sound like the teammate (research, no code yet; 21 Oct 2026)

- **OpenAI custom voices:** a voice from a recording is "limited to eligible customers" (contact sales), so it is not self-serve on the existing key. It needs a consent recording of an exact phrase and a 10 to 30 second sample (at least 5 s of speech). Voices made from a text description are free and open to all, but only work in the new GPT-Live product, not the plain speech API, and cannot imitate a person.
- **Other providers clone from short audio:** Cartesia from about 10 s, Resemble from about 5 s, Inworld from 3 to 15 s, ElevenLabs instant clone recommends 1 to 3 minutes. These need a new account and key, and a new text-to-speech client behind the provider trait.
- **What the three clips give:** about 17 s of speech, enough for the zero-shot providers but not for a stable clone. A cloned voice copies tone, pace and energy, so the extra recordings should be in the sassy delivery wanted; the 56 lines from `--voice-lines list` (about 3 minutes) would also serve as the cloning material.
- **Consent:** providers ask for the speaker's consent; the user says the teammate agrees. Their consent should be a recording or message, not assumed.
- **Not decided:** which provider. Sarvam has no cloning, so this means a second speech provider, with Hindi staying on Sarvam.

## D6. Free-tier providers for development (planning)

- **What:** Plan to use Sarvam (Saaras V4 STT, Bulbul TTS) on its ₹100 signup credit and Groq's free tier for the LLM, all behind provider traits.
- **Why:** Free to start; Sarvam supports Hindi-English code-mixing. Groq's free tier caps at about 8,000 tokens per minute, so accessibility-tree prompts must stay small.
- **Alternatives:** Cerebras ($5 trial credit that expires in 30 days), Gemini (free-tier limits unclear and changing).
