# Changelog

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
