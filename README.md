# Heyra (desktop)

Hold **fn**, talk, let go: your words are pasted where your cursor is.
Speech becomes text on your own machine with NVIDIA's Parakeet model. Nothing leaves it.

Proof of concept, macOS first. Built with [GPUI](https://www.gpui.rs) and
[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

## Run

1. Download the model (about 490 MB):
   ```sh
   mkdir -p ~/.local/share/heyra-local && cd ~/.local/share/heyra-local
   curl -L https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2 | tar xj
   ```
2. Build and open: `./bundle.sh && open target/Heyra.app`
3. Allow Microphone, and turn Heyra on under System Settings → Privacy & Security →
   Accessibility. Restart the app.
4. Hold fn, talk, let go.

A Teenage Engineering TING works too via [ting-wispr](https://github.com/alcun/ting-wispr):
its squeeze sends ctrl+opt+F12, which Heyra also listens for.

`heyra --file clip.wav` prints a transcript, to check the engine.
