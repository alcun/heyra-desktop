# Changelog

## 0.6.0

- Parakeet v2 joins the voice models: English, best of all on meetings and calls on
  heyra-bench, ahead of Parakeet v3 and Bark Clean word for word.
- Voice model and Settings… in the menu bar menu.
- Settings regrouped: push to talk, voice model, mic and open at login first, then
  Dictation, Output and Appearance. Plain labels.
- While dictating: do nothing, mute, or lower the volume to a fifth.
- The orb can sit at the top, under the menu bar; the words, toasts and copy card follow it.
- Dictionary as boxes: when I say this, write that. Tab, Enter, Esc, ⌘V; saves as you type.
- History shows the app each take went into, the model, its length and the time to text.
- Version and Check for updates at the bottom of the sidebar.
- Setup: shorter, plainer steps; it asks for Accessibility once, then opens the pane,
  and opens Home when it's done.
- New installs start on Bark Clean even if a Parakeet copy is already on the Mac.
- Fixed: macOS could close Heyra as idle when no window was open.
- The engine can run Whisper, Canary, Moonshine, Qwen3-ASR, Cohere Transcribe and NeMo
  CTC models from a folder of your own.

## 0.5.4

- Labels in sentence case, in the interface's own face: no more spaced-out capitals
  in the sidebar, settings, cards, buttons or the copy card.

## 0.5.3

- Home's status is a window into space: the orb itself, drifting, alive when you talk,
  with your mic and model as pills that open menus.
- Settings in iOS-style groups, with the microphone and voice model as menus.
  History shows every take in full.
- Theme at the top right: Dark, Light or Match the Mac.
- Tooltips spring in. Rows highlight with a rounded pill.
- An × on the words above the dot turns them off.
- After the download, a model says SETTING UP while it gets ready.
- Fixed: clicking the dot's panels could keep the keyboard, so takes didn't paste.
- Fixed: a thin white line above the words box on dark screens.

## 0.5.2

- Heyra keeps itself up to date: once a day it checks for a new version and, if you
  installed it with Homebrew, upgrades and restarts into it by itself. No terminal.
- Home in pictures: today's words over a fortnight of bars, your streak as the last
  seven days, your pace against typing.
- Tooltips that show at once: beside the sidebar icons, above bars and days.

## 0.5.1

- A new look: a top bar, a sidebar that folds down to icons, more room and fewer words.
  New type, with titles in the accent colour.
- Light and dark: Settings → Appearance (Dark, Light or Match Mac).
- Home shows today's words, time saved against typing, your streak and your pace.
- Hover the dot: Dictate, with the fn key. The pointer turns to a hand over it.
- After a `brew upgrade`, Heyra restarts into the new version by itself, between takes.
- With nowhere to type (Finder in front, no text field), your words wait above the dot
  with a Copy button.
- The words above the dot sit in two clean lines.
- A bigger window. The version is at the foot of Settings.

## 0.5.0

- Heyra writes while you talk, so your text is ready about half a second after you let go,
  however long you spoke. A 90-second take used to take 7 seconds.
- Your words appear above the dot as you talk, and the finished text stays a moment once
  it's pasted. Turn it off in Settings → Show the words while you talk.
- A bigger, roomier window. Settings starts with the voice model and says less.
- Smoother scrolling, and less work while Heyra sits idle.
- Updating from before 0.4.0: Heyra offers the new voice models once.
- Fixed: a new install could switch to Parakeet on its second launch.
- The logo is an h now, and the resting dot has a fine gold ring.

## 0.4.0

- Choose your voice model on the welcome screen or in Settings: Bark 3.0 Clean (English,
  without the "um"s), the default on a Mac set to English; Fluid Speech Mini (English,
  every word); or Parakeet v3 (every word, 25 languages). Or point Heyra at a folder
  with a sherpa-onnx model of your own.
- Switching downloads the new model while the current one keeps working. A dropped
  download resumes where it stopped and retries by itself.

## 0.3.11

- Fixed: some takes came back empty, or missed whole sentences. The speech engine is now
  sherpa-onnx 1.13.8.
- Fixed: long hands-free takes lost words. They now go in 15-second pieces.

## 0.3.10

- Fixed a freeze: clicking the dot while another app was active could lock Heyra up.

## 0.3.9

- Voice commands: "scratch that" undoes the last take; "new line" and "new paragraph" break
  the text. In a terminal (Terminal, iTerm, Ghostty, Warp and others) a take doesn't end in
  a full stop.
- Paste last take, from the menu bar.
- Settings → Keep the mic ready. Off, the mic opens only while you talk, so the orange mic
  light goes out. Plugged-in mics, like the FX mic, always stay ready.

## 0.3.8

- Notarized by Apple, so it opens without a warning.

## 0.3.7

- Welcome: the last step is two, "You're ready" and "Everything stays here".

## 0.3.6

- Welcome: the last step's lines no longer wrap around the fn key; clearer wording.

## 0.3.5

- Welcome: fn drawn as the key (🌐 fn), plainer wording, and the last step mentions
  History and choosing another key in Settings.

## 0.3.4

- Set up an FX mic (TING) from Heyra: plug it in over USB-C, then Settings → Set up this FX
  mic. Its disk's files are backed up to Documents first. No script or other app needed.
- Welcome ends with "You're ready": how to use Heyra, then a note above the dot it lives in.
- "Didn't catch that" above the dot when a take has no words, instead of nothing.

## 0.3.3

- The microphone opens as soon as it's allowed, while the voice model is still downloading,
  so the welcome window's orb follows your voice straight away.

## 0.3.2

- Welcome: "Allow microphone" asks macOS straight away and moves on with the answer
  (it waited for the voice model before, and didn't notice the answer).

## 0.3.1

- A welcome window on first run: the orb grows from the dot, then one step at a time
  (microphone, Accessibility, the fn key), each with a line on why, and a first try.
  Steps already done are skipped, and permission prompts wait until their step.

## 0.3.0

- TING built in: with its line-in as the microphone, the squeeze is push-to-talk (a double
  squeeze is hands-free), the bottom button presses Enter and the middle one undoes. No
  tingle needed. The decoder is a port of tingle's (MIT).
- Another button: choose any key, key combo or extra mouse button as push-to-talk, alongside fn.
- Microphones: a newly plugged-in mic is switched to (an adapter's line-in first), and
  Heyra falls back to the default when one is unplugged. A phone or headphones coming into
  range don't take over.
- A note above the dot when the mic changes or a TING is first heard.
- Settings lists mics with real ones first, BUILT-IN and TING tags, and a live level.
- Esc throws a take away.
- End a take with "press enter" and Heyra presses it after pasting.
- Settings: sounds on or off; mute the Mac's sound while you talk.
- Signed with a Developer ID and the hardened runtime.

## 0.2.0

- Install with Homebrew: `brew install --cask alcun/tap/heyra`.
- Hands-free: double-tap fn to start; tap fn or ✓ to paste, ✕ to keep the take in History only.
- Soft sounds when a take starts and ends.
- Takes of up to 30 minutes, written a minute at a time.
- Opens at login (Settings can turn it off).

## 0.1.0

- First release: hold fn, talk, let go. Parakeet TDT 0.6B v3 on the Mac, the orb, History,
  Dictionary, microphone choice.
