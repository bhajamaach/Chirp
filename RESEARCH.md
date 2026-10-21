# Chirp: Viability Research

*Date: 21 Oct 2026 · Status: research notes for the project record*

## 1. Purpose and scope

This document records the research done on 21 Oct 2026 to decide whether Chirp is a good idea and viable to build. Chirp is a voice-first assistant for macOS that lets blind, low-vision and motor-impaired people operate their computer by speaking Hindi, English or a mix, with spoken confirmation before anything risky.

Inputs: the PRD (v0.1), `AGENT_BUILD_BRIEF.md`, the mascot states sheet, and web research on prior studies, competitors, Apple's roadmap and Hindi speech recognition. Section 9 lists every source and says what was and was not verified.

Local environment at the time: macOS 26.5.1, arm64, Rust 1.95, Node 24, pnpm 10. No provider API keys set.

## 2. Verdict

- **Good idea:** yes. Published research shows generic computer-use agents work poorly for blind users, and blind users ask for exactly the safety behaviour Chirp is designed around.
- **Technically viable:** yes. Layers 0 and 1 (AppleScript plus the accessibility tree) cover the three v0.1 demo tasks and avoid the screenshot-grounding failures the studies describe.
- **Viable as a product:** uncertain. The risks are who the users are and which OS they use, not whether it can be built. Apple is moving into natural-language voice control, other teams are building similar tools, and no user interviews have been done.
- **Recommendation:** build v0.1 as a prototype, interview 2 or 3 users early, and position on Hindi and Hinglish, safety-first confirmation and screen reader coexistence.

## 3. Evidence of need

**A11y-CUA (CHI 2026).** A dataset of blind and low-vision users and sighted users doing 60 everyday tasks (40.4 hours, 158,325 events). A state-of-the-art computer-use agent was compared against those traces:

| Condition | Task success |
|---|---|
| Default (sighted-style interaction) | 78.3% |
| Keyboard-only (assistive-technology stand-in) | 41.67% |
| Magnifier | 28.3% |

Blind and low-vision users are keyboard-dominant while sighted users are mouse-dominant, so they cannot easily monitor or share control with an agent built around pointing and clicking.

**OLLA diary study (Sept 2026).** Three weeks, 8 blind screen-reader users, 1,258 commands, using a screen-reader-accessible computer-use agent prototype. GPT-5 scored highest, followed by Claude Sonnet, Gemini 2.5 CU, UI-TARS and Qwen3-VL. The gap between GPT-5 and Claude Sonnet was not statistically significant. The paper's conclusion is that current agents "complete a meaningful subset of nonvisual computer tasks but remain unreliable". Failure types were UI grounding, reliance on prior knowledge, multi-step planning, constraint tracking and termination. In one example the agent wrongly concluded that Word lacked a custom-margin option that was available.

**What users asked for.** Participants wanted agents to understand interfaces before acting, give situated help, explain planned actions, ask for missing information and confirm assumptions before consequential actions such as changing settings, modifying or deleting files, or handling sensitive data. One participant (P7): "I want it to ask me before clicking something important. I do not want it making decisions on its own." This matches Chirp's confirmation gate, risk levels and action log.

**Caveat:** I could not read the exact OLLA success percentages. The HTML version of the paper stripped the numbers.

## 4. Landscape: Apple

| Item | What Apple announced | Relevance to Chirp |
|---|---|---|
| Voice Control, natural language (May 2026) | Users describe on-screen controls in their own words ("tap the purple folder") instead of exact labels or numbers. The press release frames it around iPhone and iPad. | Overlaps with Chirp's target users. Targets individual UI elements, not multi-step intents. |
| Voice Control availability | English in the U.S., Canada, UK and Australia only. | No Hindi or Hinglish. |
| Siri AI (June 2026) | On-screen awareness, personal context, systemwide app actions (for example drafting an email, adding a recipe to Notes). Developer testing now on iOS 27 and macOS 27, user beta later in 2026, English first. Apple says more languages will follow quickly. | The closest overlap with Chirp's intent-level control. Apple Intelligence's listed languages do not include Hindi. |
| macOS version | These features ship on macOS 27. The test Mac runs macOS 26.5.1. | Chirp's window to be useful before Apple catches up is short. |

Takeaway: Hindi and Hinglish speech is Chirp's defensible edge against Apple, and it is time-limited.

## 5. Landscape: other builders

I only saw search snippets or comment threads for most of these, so capabilities are unverified.

| Name | What it is | Source quality |
|---|---|---|
| Disha | An AI voice tutor that helps blind people use a laptop. Windows and NVDA. If NVDA is running, Disha has no voice of its own and speaks through NVDA's controller client, otherwise it uses the Windows SAPI voice. State changes are sounds, not words. Has stop and repeat keys. Self-reported in Google's Fund My Crazy top 50 out of 1 lakh+ entries. Its builders say the order of screen reader announcements versus Disha's own is "the unsolved bit". | Reddit thread, read in full |
| Oruk | A project by a Reddit commenter, also concerned with spoken guidance overlapping a screen reader. | One comment |
| macOS AI assistant for visually impaired users | A founder recruiting 10 free testers in a Facebook group about 7 months ago. | Search snippet only |
| Vellum | Open-source personal assistant with a native Mac app. Reported to have an "Ambient Assistants" floating pill over any Mac app that sees the screen. | Search snippets only |
| Generic computer-use agents | OpenAI Operator, Anthropic computer use, Microsoft Copilot, Gemini computer use. Screenshot-based. | Named in the OLLA paper |

One Reddit commenter also said Codex computer use already covers a similar use case. That is one person's opinion and I did not check it.

## 6. Technical feasibility

**Hindi-English speech recognition.** Sarvam's Saaras V4 (announced 25 Sept 2026) claims:

- 22 Indian languages plus English, with streaming over WebSocket and time to first token under 150 ms.
- Five output modes from one model: transcribe, translate, verbatim, translit and codemix (native script with English words kept in English).
- Language identification error of 2.9% across the ten most spoken Indian languages and 5.22% across all 22.
- Keyterm prompting to bias toward names and terms. SDKs for Python and Node.js, with Pipecat and LiveKit integrations.
- Pricing "as low as ₹3 a minute" (from a search snippet, not the page text I read).

These are the vendor's own claims. The only side-by-side transcripts shown against ElevenLabs Scribe v2, Deepgram Nova3 and GPT-4o Transcribe were Bengali. Nothing I read tests short Hindi command phrases or Hinglish commands on a laptop microphone. This must be measured in slice S2.

**Execution layers.** AppleScript and the macOS accessibility tree avoid the screenshot-grounding failures reported in both studies, but the accessibility tree has gaps. Unlabelled or custom UI exposes little, and the tree cannot describe images. The "what is on screen" demo task is limited to what the tree exposes. This point is my own analysis, not from a source.

**Known hard problems in the brief:**

- Capturing the Fn key (macOS reserves it for dictation and the emoji picker).
- Detecting "stop" while Chirp itself is speaking, which needs echo handling.
- Sharing audio with VoiceOver without talking over it (Disha lists the same problem as unsolved).
- A 3 second LLM-path latency target and a 90% success target over 20 runs per task.

## 7. Risks

| Risk | Why it matters | Mitigation |
|---|---|---|
| Wrong platform for first users | From general knowledge (not researched here), most blind screen-reader users, particularly in India, are on Windows with NVDA or JAWS. A Mac-only v0.1 may have few testers. Disha chose Windows. | Ask in interviews. The `PlatformAdapter` trait already leaves room for Windows. |
| No demand validation | The PRD calls for 2 to 3 interviews and none have happened. The three demo tasks are easy with AppleScript and prove the pipeline, not the demand. | Interview before heavy investment past S1. |
| Apple catches up | Natural-language Voice Control and Siri AI are shipping on macOS 27. | Differentiate on Hindi and Hinglish, safety and screen reader coexistence. |
| Competing builders | Disha, a Mac assistant for blind users, Vellum and generic agents. | Verify competitors' real capabilities before positioning. |
| Screen reader coexistence unsolved | Two voices can hide the instruction the user needs next. | Test with real screen reader users early. Consider speaking through the screen reader. |
| Scope and targets | 90% success over 20 runs on unfamiliar apps is aggressive. | Keep to the three demo tasks. Report measured numbers honestly. |
| Privacy | Transcripts and UI data go to cloud models. Free-tier hosts can log prompts. | Fake data only in development. Show what leaves the device. |

## 8. Recommendations and next steps

1. Build v0.1 as a prototype, following the brief's slices S0 to S7.
2. Interview 2 or 3 blind or low-vision users, ideally in India. Ask which OS and screen reader they use and what they would actually ask Chirp to do.
3. Position on Hindi and Hinglish, safety-first confirmation and screen reader coexistence.
4. Decide early whether Windows and NVDA should come before or alongside macOS.
5. Measure Hindi and Hinglish command accuracy and latency with real phrases in slice S2, before relying on any vendor claim.
6. Verify competitors (Disha, the Mac assistant for blind users, Vellum) by trying them, not from snippets.

Open questions from the build brief that this research does not answer: which provider keys exist, which screen reader the first users run, whether Fn is capturable on the target Mac, and whether the test Mac can be driven for Notes, Finder and TextEdit.

## 9. Sources and limitations

**Pages opened and read:**

- Apple, [new accessibility features and updates with Apple Intelligence](https://www.apple.com/newsroom/2026/05/apple-unveils-new-accessibility-features-and-updates-with-apple-intelligence/) (19 May 2026)
- Apple, [Siri AI announcement](https://www.apple.com/newsroom/2026/06/apple-introduces-siri-ai-a-profoundly-more-capable-and-personal-assistant/) (8 Jun 2026)
- Sarvam, [Introducing Saaras V4](https://www.sarvam.ai/blogs/introducing-saaras-v4) (25 Sept 2026)
- arXiv, [A11y-CUA Dataset: Characterizing the Accessibility Gap in Computer Use Agents](https://arxiv.org/abs/2602.09310) (abstract only; the ACM page blocked the fetch)
- arXiv, [Are We There Yet? Assessing Computer-Use Agents for Blind Users' Accessible Interaction with Desktop Applications](https://arxiv.org/html/2609.00524v1) (success percentages stripped from the HTML)
- Reddit r/hackathon, [Disha thread](https://www.reddit.com/r/hackathon/comments/1wot531/we_built_an_ai_agent_that_lets_blind_people_use_a/)

**Seen only as search snippets (not opened):** Facebook group post about the macOS assistant for visually impaired users, Vellum's site and blog, the Vellum "Ambient Assistants" post on X, MacRumors and Macworld coverage of Apple's Voice Control, Sarvam's pricing line, and a Hume AI article on voice control.

**Limitations:**

- Vendor claims (Sarvam, Vellum) are unverified by me.
- Competitor capabilities were not tested.
- The claim about Windows and NVDA dominance among blind users is from general knowledge and was not researched here.
- No user interviews have been done, so demand is still a hypothesis.
- Research reflects what was found on 21 Oct 2026 and will date quickly given how fast Apple and the agent space are moving.
