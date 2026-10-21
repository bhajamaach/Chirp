# Chirp

A voice-first assistant for macOS for blind, low-vision and motor-impaired users. See `CHIRP.md` for what it is and how it is built, `PRD.md` for the product, and `AGENT_BUILD_BRIEF.md` for the v0.1 build plan.

Status: early build. Slices S0 (menu-bar app and overlay), S1 (permissions, talk key), S2 (microphone and speech-to-text), most of S3 (spoken replies, sound cues, interrupt) S4 (the macOS accessibility adapter) and S5 (turning a spoken request into actions) exist. Hold the talk key and ask: simple commands run at once, and anything else is planned and done step by step. Irreversible actions are read back and run only after a spoken yes (S6).

## Setup

Requires macOS on Apple Silicon, Rust, Node and pnpm.

```sh
pnpm install
cp .env.example .env   # then put your Sarvam key in SARVAM_API_KEY
pnpm tauri dev
```

`.env` is git-ignored. Never commit keys.

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

## Speech-to-text

Hold the talk key, speak Hindi, English or a mix, and let go. The mascot shows Listening while you hold, Thinking while the transcript comes back, then Done. The menu shows what Chirp heard, and the terminal logs the transcript and the stage timings. Nothing is saved to disk except a count of seconds used.

Test without speaking by running a recorded clip through the same path:

```sh
cd src-tauri
cargo run -- --transcribe-file path/to/clip.wav
# make a test clip with the built-in voices:
#   say -v Lekha -o clip.aiff "नोट्स खोलो" && afconvert -f WAVE -d LEI16@16000 -c 1 clip.aiff clip.wav
```

## Speech, sounds and interrupting

Chirp speaks through Sarvam's Bulbul voice. Fixed phrases are saved in `~/Library/Caches/com.chirp.app/` after the first time, so they cost credit once and then start instantly; if the key, network or credit is missing, Chirp uses the macOS voice instead. Every state also has a short sound: a rising tone when it starts listening, a soft tick while it thinks, two notes when done, a low falling tone on an error (`CHIRP_CUES=0` turns them off).

After each command Chirp reads back what it understood ("You said: open Notes") in the free macOS voice, because it cannot act on commands yet; `CHIRP_READBACK=off` turns that off.

**To interrupt Chirp, press the talk key.** It stops at once and starts listening. Speaking over Chirp is built as an experiment but is **off by default**, because on laptop speakers it failed its real-room test (it interrupted Chirp itself). With headphones there is no echo to tell apart, so it may work; try `CHIRP_BARGE_IN=1`, which is untested.

Test commands (each uses your speakers, and the first run of a new phrase costs a little credit):

```sh
cd src-tauri
cargo run -- --speak "All set."     # speak one line (120 characters at most)
cargo run -- --stop-test            # measure how fast playback stops (free)
cargo run -- --cue-test             # play each sound cue and measure it on the microphone (free)
cargo run -- --barge-test           # experimental speak-over check (about 25 s, audible; currently fails on speakers)
```

### Credit

Sarvam's free credit is Rs 100. Listening costs Rs 30 per hour of audio sent and speaking costs Rs 30 per 10,000 characters (about Rs 0.12 for a 40-character line, once, since fixed phrases are cached). Chirp keeps one estimate for both, stops paid calls at Rs 90 (`SARVAM_BUDGET_RUPEES`), caps one recording at 20 seconds (`CHIRP_MAX_UTTERANCE_SECS`), ignores presses under 0.3 seconds, and never re-sends audio or text after a failure. The estimate is in `~/Library/Application Support/com.chirp.app/usage.json`. The Sarvam dashboard shows the real balance.

## Asking Chirp to do things

Hold the talk key and speak in Hindi, English or a mix.

- **Simple commands run with no model:** "open Notes", "नोट्स खोलो", "scroll down", "go back", "what app am I in". These work even with no model key.
- **Everything else goes to a language model**, which breaks the request into subtasks and does them one action at a time while Chirp narrates: "Open Notes and write my shopping list: milk, eggs, bread." The model is chosen from your `.env`: `OPENAI_API_KEY` first, then Sarvam's. See `.env.example`. Your request and a short outline of the front app's window are sent to that host.
- **If it needs one fact,** it asks one short question; hold the key and answer.
- **Before anything that can't be undone** (sending, deleting, moving files, submitting, overwriting) it says what it is about to do and asks "Is that okay?" **Hold the talk key and say yes or no.** Only a clear yes ("yes", "go ahead", "haan", "ठीक है") lets it go ahead; no, silence or anything unclear means no. It asks again for each such step.
- **If it can't do something,** it says so and stops. It never presses payment buttons, opens System Settings or runs shell scripts.
- **"What did you just do?"** reads back its last task from the action log (also "तुमने अभी क्या किया"). The log (`~/Library/Application Support/com.chirp.app/actions.jsonl`) holds actions, never what you typed or said.
- **You can just talk to it:** say hello, ask what it can do, or follow up ("also add bread", "do that again"). It remembers the last few exchanges for ten minutes, in memory only.
- **To stop it,** press the talk key. It stops speaking and before its next step.

Try it from a terminal, without speaking (`--run` really does the task, so it will open and type in your apps):

```sh
cd src-tauri
cargo run -- --plan "Open Notes and write my shopping list: milk, eggs, bread"   # shows the plan, touches nothing, sends no screen text
cargo run -- --run "open Notes"                                                   # fast path, no model
cargo run -- --run "Open Notes and write my shopping list: milk, eggs, bread" --speak
```

## Controlling apps (typed test commands)

The adapter reads other apps' accessibility trees and drives them. Until S5 connects it to your voice, you try it with typed commands. From `src-tauri`, with the app already built:

```sh
cargo run -- --ax frontmost                          # which app is in front
cargo run -- --ax focus Notes                        # open or switch to an app
cargo run -- --ax outline --app Notes                # short text summary of the window
cargo run -- --ax tree 6 --app Finder                # full tree to a depth
cargo run -- --ax find "new note" --app Notes        # find by label, best match first
cargo run -- --ax press "new note" --role button --app Notes
cargo run -- --ax type "milk, eggs, bread"           # types into whatever has focus
cargo run -- --ax keys cmd+n                         # a shortcut
cargo run -- --ax script 'tell application "Finder" to get name of startup disk'
cargo run -- --ax selftest [textedit|finder|notes]   # the S4 acceptance test
```

`--app` picks an app by name; without it the app in front is used, which from a terminal is the terminal. `type` and `keys` always go to the app in front, so run `focus` first. The self-test opens a new TextEdit document, a Finder window and a new note, with fake text, and cleans up after itself (it deletes only a note whose title starts with "Chirp S4 test"). Do not click or switch apps while it runs.

## Personality

Chirp is sassy by default: quick quips, short lines, a bit of swagger, never at the user's expense, and plain and clear for confirmations, errors and refusals. Turn it down with `CHIRP_PERSONALITY=warm` or `plain` in `.env`.

## Recorded voice lines

Chirp's fixed phrases ("On it.", "Is that okay?", the setup lines, error and refusal lines: 50 in English) can be played from recordings of a real person instead of the synthetic voice. They start instantly and cost no credit.

```sh
# from the project root (pnpm chirp runs the Rust program; --silent keeps pnpm's own lines out of the output)
pnpm --silent chirp --voice-lines script > scripts.txt             # the plain-language script to hand to the person recording
pnpm --silent chirp --voice-lines list                            # the same lines with their technical file names
pnpm --silent chirp --voice-lines import ~/Downloads/lines        # a folder of wav, mp3, m4a or aiff files
pnpm --silent chirp --voice-lines status                          # which lines are still missing
pnpm --silent chirp --speak "On it."                              # hear one
```

`scripts.txt` in the project root is the current script to hand over (regenerate it after the list of lines changes, since files are named by line number). Anything shown above as `cargo run -- ...` can also be run as `pnpm --silent chirp ...` from the root.

Files are matched by their number in the script (`14.m4a`), by `on-it.wav`, or by the words themselves (`On it.wav`). Import trims the silence at both ends and evens out the loudness. Recordings go in `~/Library/Application Support/com.chirp.app/voice-lines/`. Lines the model writes (results, questions, chat) are still spoken in the synthetic voice.

## Required permissions

Chirp asks for these one at a time, with a spoken reason:

- **Microphone:** to hear you (used from S2).
- **Accessibility:** to control other apps, and to swallow the talk key so it does not type.
- **Input Monitoring:** to detect the talk key.
- **Automation (asked by macOS the first time Chirp scripts each app):** to run AppleScript in Finder, Notes and the like. macOS asks once per app, and the dialog blocks the keyboard until you answer it. If you chose Don't Allow, turn it back on in System Settings, Privacy & Security, Automation. This one is not part of the spoken setup yet.
- Screen Recording is only needed if the vision fallback is enabled, which v0.1 does not build.

When you run `pnpm tauri dev`, macOS lists the app that launched it (Terminal, iTerm, VS Code and so on) in these lists instead of Chirp. If Input Monitoring is granted but the key still does nothing, quit and relaunch.

## Choosing a talk key

Right Command, Right Option, Right Control, Right Shift and F1 to F20 work. Typing keys, left-side modifiers, arrow keys, Escape and Caps Lock are refused because holding them would break normal typing. Fn or Globe only works if System Settings, Keyboard has "Press 🌐 key to" set to "Do Nothing".

## Environment variables

See `.env.example`.
