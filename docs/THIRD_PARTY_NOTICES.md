# Third-party notices

Vaporly ships with, links against, or downloads the following third-party
software. Each project remains under its own license; full texts are in the
linked repositories, in bundled dependency metadata, and (for the items called
out below) in this file.

| Project                                                                                           | Use                                                                                                                                                                       | License                                                                                                            |
| ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| [Handy](https://github.com/cjpais/Handy) by CJ Pais                                               | The foundation Vaporly is built on                                                                                                                                        | MIT (full notice below, under "Handy (the upstream project)")                                                      |
| [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp)                                | Speech-to-text engine                                                                                                                                                     | MIT                                                                                                                |
| [whisper.cpp / GGML](https://github.com/ggml-org/whisper.cpp)                                     | Whisper-family inference                                                                                                                                                  | MIT                                                                                                                |
| [llama.cpp](https://github.com/ggml-org/llama.cpp)                                                | Bundled local cleanup engine (llama-server)                                                                                                                               | MIT (copy shipped as LICENSE.llama.cpp beside the binary)                                                          |
| [Silero VAD](https://github.com/snakers4/silero-vad)                                              | Voice activity detection model, bundled as `silero_vad_v4.onnx`                                                                                                           | MIT                                                                                                                |
| [Parakeet TDT 0.6B v2](https://huggingface.co/handy-computer/parakeet-tdt-0.6b-v2-gguf) by NVIDIA | The speech-to-text model, downloaded from Hugging Face on first run                                                                                                       | CC-BY-4.0, attribution required (see the `license` field in [catalog.json](../src-tauri/src/catalog/catalog.json)) |
| [Qwen2.5 models](https://huggingface.co/Qwen) by Alibaba                                          | Cleanup language models, downloaded from Hugging Face on demand                                                                                                           | Apache-2.0                                                                                                         |
| [dolph/dictionary `popular.txt`](https://github.com/dolph/dictionary)                             | Embedded common-English wordlist (~25k words, lowercased and sorted into `src-tauri/src/auto_learn/common_words_en.txt`) that stops auto-learn from saving ordinary words | MIT                                                                                                                |
| [Manrope](https://github.com/sharanda/manrope)                                                    | The bundled UI font (Latin-subset variable woff2 in `public/fonts/`)                                                                                                      | SIL OFL 1.1 (full text in [public/fonts/OFL.txt](../public/fonts/OFL.txt))                                         |
| [Sora](https://github.com/wildbit/sora)                                                           | The bundled brand-wordmark font (Latin-subset variable woff2 in `public/fonts/`)                                                                                          | SIL OFL 1.1 (full text in [public/fonts/OFL.txt](../public/fonts/OFL.txt))                                         |
| Sound themes (`src-tauri/resources/*.wav`)                                                        | Recording start and stop feedback sounds                                                                                                                                  | marimba and pop derived from Handy (MIT); chime, bubble, and breeze original to Vaporly (AGPL-3.0)                 |
| System GTK3 and gtk-layer-shell (Linux builds only)                                               | Overlay windowing on Linux, linked dynamically at runtime                                                                                                                 | LGPL-2.1-or-later (the Rust `gtk` and `gtk-layer-shell` crates are MIT)                                            |
| Rust and JavaScript dependencies                                                                  | Application runtime                                                                                                                                                       | Per-crate/package licenses (see Cargo.lock / bun.lock metadata)                                                    |

Thank you to all of these projects.

## Handy (the upstream project)

Vaporly is a fork of Handy by CJ Pais (https://github.com/cjpais/Handy). Handy
is distributed under the MIT License, which requires its copyright notice and
permission notice to be retained in distributions. That notice is reproduced
here in full:

```
MIT License

Copyright (c) 2025 CJ Pais

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Sound themes

Vaporly bundles five recording feedback sound themes as WAV files in
`src-tauri/resources/`:

- marimba and pop are derived from the Handy project and are used under the
  MIT License (see the Handy notice above).
- chime, bubble, and breeze are original works created for the Vaporly project
  and are covered by Vaporly's own AGPL-3.0 license.

## Fonts

Vaporly bundles two fonts as Latin-subset variable woff2 files in
`public/fonts/`. Both are licensed under the SIL Open Font License, Version 1.1.
The complete license text, including the required copyright and Reserved Font
Name headers, is in [public/fonts/OFL.txt](../public/fonts/OFL.txt).

- Manrope. Copyright (c) 2018 The Manrope Project Authors
  (https://github.com/sharanda/manrope), Reserved Font Name "Manrope".
- Sora. Copyright (c) 2020 The Sora Project Authors
  (https://github.com/wildbit/sora), Reserved Font Name "Sora".

## Linux system libraries

On Linux, Vaporly's overlay window is drawn with GTK3 and gtk-layer-shell.
These system libraries are linked dynamically at runtime and are provided by
your operating system, not bundled by Vaporly. They are licensed under the
GNU Lesser General Public License, version 2.1 or later (LGPL-2.1-or-later).
This use is standard dynamic linking, which is compatible with Vaporly's
AGPL-3.0 license. The corresponding Rust crates (`gtk`, `gtk-layer-shell`)
are themselves MIT licensed.
