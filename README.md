<p align="center"><img src="docs/banner.webp" alt="Heyra: free, open source voice to text that never leaves your Mac" width="100%"></p>

<p align="center">Hold fn, talk, let go. It types where your cursor is.</p>

<p align="center">
  <a href="https://github.com/alcun/heyra-desktop/releases/latest">Download</a> ·
  <a href="https://heyra.alcun.dev">Website</a> ·
  <a href="https://heyra.alcun.dev/bench">Bench</a>
</p>

```sh
brew install --cask alcun/tap/heyra
```

**0.5 s** from letting go to text · **6.1%** of words wrong, off the cuff · **25** languages · **0 bytes** of your voice leave your Mac

Heyra runs the voice model on your own Mac. It goes online once, to download the model,
and after that it works with Wi-Fi off. No account, no subscription, MIT licensed.
Apple Silicon only, for now.

![Heyra's Home with the voice model menu open](docs/home.webp)

## Install

With [Homebrew](https://brew.sh), as above. No admin rights on a work Mac? Install
into your own Applications folder instead, for this and every upgrade:

```sh
echo 'export HOMEBREW_CASK_OPTS="--appdir=~/Applications"' >> ~/.zshrc && source ~/.zshrc
brew install --cask alcun/tap/heyra
```

Or download `Heyra.zip` from [Releases](https://github.com/alcun/heyra-desktop/releases),
unzip it and move Heyra to Applications. It isn't notarized by Apple yet, so the first
time macOS blocks it: System Settings → Privacy & Security → Open Anyway.

Open Heyra. Home shows a short setup list and ticks each step off as you go:

1. The voice model downloads, about 400 MB, once
2. Allow the microphone
3. Turn Heyra on under Privacy & Security → Accessibility, so it can hear fn and type
4. Set Keyboard → Press 🌐 key to → Do nothing, so fn doesn't also open emoji

Then click into any text box, hold fn, say something and let go.

Installed with Homebrew, Heyra keeps itself up to date: once a day it checks for a new
release, upgrades through Homebrew and restarts into it when you're not talking.

## Using it

Heyra lives in the menu bar and as a small dot above the Dock. Hold fn and the dot
grows into the orb. Your words show above it while you talk, and when you let go they're
pasted where your cursor is, with your clipboard put back.

<img src="docs/words-box.webp" alt="The words box, showing a sentence as it's spoken" width="560">

If there's nowhere to type, Heyra keeps the take on a card for you to copy.

<img src="docs/copy-card.webp" alt="A card reading Nowhere to type, with the take and a Copy button" width="460">

Press another key while you hold fn, or Esc, to cancel a take. Double-tap fn to talk
hands-free, and tap it again to stop. Any key, key combo or extra mouse button can be
push-to-talk too, in Settings.

Say "press enter" at the end of a take and Heyra presses it after pasting. "New line"
and "new paragraph" break the text, and a take of just "scratch that" undoes the last
one. In a terminal, takes don't end in a full stop.

Every take is kept in History on your Mac; click one to copy it again. The Dictionary
fixes words it mishears, like `a cappy bar => capybara`. Plug in any microphone and
Heyra switches to it. Takes can run to 30 minutes.

Heyra writes while you talk, so the wait after you let go stays around half a second
however long you spoke. At rest it uses about 2% of one CPU core. The model stays
loaded, about 1.3 GB of memory, so it's ready the moment you press fn.

A Teenage Engineering EP-2350 works as a push-to-talk mic with nothing else installed:
choose its line-in, squeeze to talk, and the bottom button presses Enter. Settings → Set
up this FX mic puts a small script and four tones on it, from
[ting-wispr](https://github.com/alcun/ting-wispr), after backing up its files.

Everything Heyra keeps is in `~/Library/Application Support/Heyra`.

## Voice models

Pick one on Home or in Settings. Switching downloads the new model while the current
one keeps working, and every model is checked against a SHA-256 before use.

| Model | Size | For | Average | Off the cuff |
|---|--:|---|--:|--:|
| Bark 3.0 Clean | 399&nbsp;MB | English, leaves out "um", "uh" and false starts | 9.9 | **6.1** |
| Fluid Speech Mini | 312&nbsp;MB | English, every word | **6.6** | 17.3 |
| Parakeet v3 | 487&nbsp;MB | English and 24 European languages | 8.1 | 17.5 |

Words wrong in 100, fewer is better. "Off the cuff" is scored against what the speaker meant.
Measured with [heyra-bench](https://github.com/alcun/heyra-bench) on 800 public clips;
the full results are on the [Bench](https://heyra.alcun.dev/bench). Bark Clean's
average is higher because those transcripts keep every "um", which it leaves out on
purpose. You can also use your own model: Settings → Use a model folder, with any
sherpa-onnx transducer (encoder, decoder and joiner `.onnx` files and `tokens.txt`).

Bark 3.0 Clean ([Underdog](https://huggingface.co/ConwayResearch/Underdog-Bark-3.0-Clean))
and Fluid Speech Mini ([FluidVoice](https://huggingface.co/FluidVoice-Labs/fluid-speech-mini-onnx))
are fine-tunes of NVIDIA's Parakeet TDT 0.6B v3, converted for sherpa-onnx.

## Build from source

You need Rust (`rustup`) and Xcode's command line tools.

```sh
git clone https://github.com/alcun/heyra-desktop.git
cd heyra-desktop
./bundle.sh            # builds target/Heyra.app
open target/Heyra.app
```

`bundle.sh` signs with your Apple Development certificate if you have one, so macOS
keeps your permissions between builds. Without one it signs ad hoc, and macOS asks for
Accessibility again after each rebuild. `target/release/heyra --file clip.wav` prints a
transcript, to test the engine.

The interface is [GPUI](https://www.gpui.rs), the framework Zed is built with.
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) runs the model in the app itself,
int8 on the CPU. A macOS event tap watches for fn, `cpal` records the microphone, and
the text is pasted with a real ⌘V.

## Licence

Heyra is MIT licensed. The models, runtime and fonts have their own licences; see
[NOTICE](NOTICE).
