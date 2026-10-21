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

## D6. Free-tier providers for development (planning)

- **What:** Plan to use Sarvam (Saaras V4 STT, Bulbul TTS) on its ₹100 signup credit and Groq's free tier for the LLM, all behind provider traits.
- **Why:** Free to start; Sarvam supports Hindi-English code-mixing. Groq's free tier caps at about 8,000 tokens per minute, so accessibility-tree prompts must stay small.
- **Alternatives:** Cerebras ($5 trial credit that expires in 30 days), Gemini (free-tier limits unclear and changing).
