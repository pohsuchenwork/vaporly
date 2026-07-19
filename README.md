<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="104" alt="Vaporly icon" />

# Vaporly

**Speak. It types.**

Free forever. 100% private. Everything runs on your own computer.

<p>
<a href="#download"><img src="docs/assets/btn-download.svg" alt="Download Vaporly" /></a>
</p>

<p>
<img src="docs/assets/badge-opensource.svg" alt="Open source, AGPL-3.0" />
<img src="docs/assets/badge-private.svg" alt="Private, on-device" />
<img src="docs/assets/badge-platforms.svg" alt="macOS, Windows, Linux" />
</p>

<p>
<a href="https://ko-fi.com/pohsuchenwork"><img src="docs/assets/btn-kofi.svg" alt="Buy me a coffee on Ko-fi" /></a>
<a href="https://paypal.me/pohsuchenwork"><img src="docs/assets/btn-paypal.svg" alt="Donate with PayPal" /></a>
</p>

Vaporly is free and always will be. If it saves you typing, a coffee keeps it going.

</div>

---

## What Is Vaporly?

Hold a key, talk, let go: your words appear as clean, polished text in whatever app you are using. No cloud, no account, no subscription. Unlike online dictation services, **your voice never leaves your computer**: the speech recognition and the AI tidy-up both run right on your machine.

## What Vaporly Can Do

- **Talk instead of type.** Hold **Fn**, speak, release: polished text lands in whatever app you are using.
- **Hands-free mode.** Double-tap the key to keep talking without holding it. **Esc** cancels.
- **See it as you speak.** Pick a live overlay: your words appearing in real time, a small status bar, or nothing.
- **Whisper mode.** Dictate quietly and Vaporly still hears you, while louder sounds around you are ignored. Calibrate it to your microphone and room.
- **Custom words.** Teach it names, brands, and jargon so they always come out spelled your way.
- **Custom phrases.** Say a short trigger and Vaporly types your saved text, like a full email template.
- **Tidy-up.** Removes "um" and "uh", and fixes mid-sentence changes of mind ("meet at eight, no wait, nine").
- **Fits the app.** An email is laid out with a greeting, body, and sign-off; long messages break into clean paragraphs; chat and notes get tidy spacing; and code stays literal.
- **History.** Every dictation saved locally, searchable and editable, with audio playback and re-transcribe.
- **Learns your words.** Optionally grows your personal dictionary from the corrections you make.
- **Your look.** Light or dark theme with a choice of accent colors.
- **Sound cues.** Gentle start and stop sounds you can change or switch off.
- **Private by design.** Speech and cleanup run on your machine. Nothing you say is sent to us or to any cloud.

## Download

<div align="center">

<a href="https://github.com/pohsuchenwork/vaporly/releases/latest/download/Vaporly_aarch64.dmg"><img src="docs/assets/btn-mac-silicon.svg" alt="Download for Mac, Apple Silicon" /></a><br />
<a href="https://github.com/pohsuchenwork/vaporly/releases/latest/download/Vaporly_x64.dmg"><img src="docs/assets/btn-mac-intel.svg" alt="Download for Mac, Intel" /></a><br />
<a href="https://github.com/pohsuchenwork/vaporly/releases/latest/download/Vaporly_x64-setup.exe"><img src="docs/assets/btn-windows.svg" alt="Download for Windows" /></a><br />
<a href="https://github.com/pohsuchenwork/vaporly/releases/latest/download/Vaporly_arm64-setup.exe"><img src="docs/assets/btn-windows-arm.svg" alt="Download for Windows on ARM" /></a><br />
<a href="https://github.com/pohsuchenwork/vaporly/releases/latest/download/Vaporly_amd64.AppImage"><img src="docs/assets/btn-linux.svg" alt="Download for Linux" /></a>

</div>

> [!TIP]
> **Not sure which Mac you have?** Click the Apple menu at the top-left of your screen and choose **About This Mac**. If the chip says "Apple", pick **Apple Silicon**; otherwise pick **Intel**.

## First-Time Setup on a Mac

> [!IMPORTANT]
> **macOS will say it "cannot verify" Vaporly. That is expected, and it is not a virus.**
> Vaporly is free software and is not registered with Apple's paid developer program, so your Mac shows a warning the first time. Here is the fix, and you only ever do it once:
>
> 1. Open your **Applications** folder and double-click **Vaporly**. When the warning appears, click **Done** (or Cancel).
> 2. Open **System Settings**, then click **Privacy and Security**.
> 3. Scroll down until you see _"Vaporly" was blocked_, and click **Open Anyway**. Confirm, and you are in.

On Windows, if SmartScreen shows "Windows protected your PC", click **More info**, then **Run anyway**. Same reason, same one-time fix.

## Allow Permissions

When Vaporly first runs, your computer asks you to allow a few things. Say yes to all three; each has one simple job:

| Permission           | Why Vaporly needs it                                |
| -------------------- | --------------------------------------------------- |
| **Microphone**       | So it can hear you                                  |
| **Accessibility**    | So it can type the text into your apps              |
| **Input Monitoring** | So the talk key works no matter what app you are in |

> [!NOTE]
> On first run Vaporly also downloads its speech model (about **700 MB**, one time only). Give it a minute on the setup screen.

## Say Your First Sentence

1. Click into any text box (a note, an email, a chat).
2. **Press and hold the Fn key** (bottom-left of most keyboards) and say a sentence.
3. Let go. Your words appear as clean text, right where your cursor is.

That is the whole thing: hold, speak, release. Everything below is optional.

---

## Will It Run on My Computer?

- **Mac:** macOS 10.15 (Catalina) or newer, Apple Silicon or Intel.
- **Windows:** Windows 10 or 11 (64-bit x64 or ARM64).
- **Linux:** a modern 64-bit distribution with GTK 3 and WebKitGTK (for example Ubuntu 22.04 or newer, or Fedora). Builds are provided as .deb, .rpm, and .AppImage.
- **Memory:** works on **4 GB** of RAM using instant, rule-based cleanup with no AI model to download. **6 GB or more** automatically adds a small on-device AI cleanup model, and **14 GB or more** uses the highest quality one. The AI cleanup models download only when you set a cleanup option to Model.
- **Storage:** about 1 GB free to start (the app plus the roughly 700 MB speech model). Up to about 5 GB if you use the largest on-device AI cleanup model.

## Install From the Terminal

These download and install the latest version for you, and skip the "Open Anyway" step on Mac:

```bash
# macOS and Linux
curl -fsSL https://github.com/pohsuchenwork/vaporly/releases/latest/download/install.sh | bash
```

```powershell
# Windows (PowerShell)
powershell -c "irm https://github.com/pohsuchenwork/vaporly/releases/latest/download/install.ps1 | iex"
```

## Security and Privacy

Vaporly is built to keep your voice and your words on your own machine.

- Speech recognition and the optional AI cleanup both run locally. The cleanup engine listens only on your own computer (`127.0.0.1`) behind a fresh per-session key.
- The only things that ever reach the internet are checking for app updates and downloading the speech and cleanup models the first time. There is no telemetry, no analytics, and no account.
- Updates are cryptographically signed, and the app respects secure input fields (it will not type into password boxes).

Full details, including the threat model and exactly what does and does not leave your machine: [Security](.github/SECURITY.md).

## Legal

- Vaporly 3.0.0 is the first official public release, under the GNU AGPL-3.0. Earlier versions were development and pre-release builds.
- License: [GNU AGPL-3.0](LICENSE). Free to use, read, modify, and share; anything built on Vaporly must stay open under the same license.
- Commercial licensing: if you want to use Vaporly in a closed-source or commercial product that the AGPL does not allow, a separate commercial license is available. Contact pohsuchenwork@gmail.com.
- Contributions require the [Contributor License Agreement](CLA.md); see [Contributing](.github/CONTRIBUTING.md).
- [Privacy Policy](docs/PRIVACY.md): what Vaporly accesses and stores (spoiler: it stays on your machine).
- [Disclaimer](docs/DISCLAIMER.md): warranty, responsible use, and trademark notes.
- [Third-party notices](docs/THIRD_PARTY_NOTICES.md) and [NOTICE](docs/NOTICE): the open-source projects Vaporly builds on.
- Provided as is, at your own risk. There is no warranty and no obligation to maintain, update, or support it, and features and policies may change at any time without notice.

Vaporly is independent and is not affiliated with or endorsed by any other company or product named in its documentation.

<details>
<summary><b>Build From Source (Developers)</b></summary>

Prereqs: [Rust](https://rustup.rs/) (stable), [Bun](https://bun.sh/), and cmake (`brew install cmake`).

```bash
bun install
mkdir -p src-tauri/resources/models
curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx

CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev     # develop
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build   # build a release app and installer
```

Platform notes and troubleshooting: [BUILD.md](docs/BUILD.md). Contributing: [CONTRIBUTING.md](.github/CONTRIBUTING.md).

</details>

## Learn More

- **[Website](https://pohsuchenwork.github.io/vaporly/)**: the download page and overview.
- **[User Guide](docs/USER_GUIDE.md)**: every screen, setting, and hotkey in plain language.
- **[Whisper Mode](docs/WHISPER_MODE.md)**: how quiet-capture and loud-rejection work.
- **[Architecture](docs/ARCHITECTURE.md)**: how Vaporly is built, front to back.
- **[Changelog](docs/CHANGELOG.md)**: what changed in each version.

## Credits

Built on [Handy](https://github.com/cjpais/Handy) by CJ Pais. Speech recognition via [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) and [whisper.cpp](https://github.com/ggml-org/whisper.cpp), cleanup via [llama.cpp](https://github.com/ggml-org/llama.cpp), voice activity detection by [Silero](https://github.com/snakers4/silero-vad). Interface icons from [Lucide](https://lucide.dev) (ISC) and [Simple Icons](https://simpleicons.org) (CC0).

<div align="center">

Made with care by one person. If Vaporly helps you, you can
<a href="https://ko-fi.com/pohsuchenwork">buy me a coffee on Ko-fi</a> or
<a href="https://paypal.me/pohsuchenwork">donate with PayPal</a>.

</div>
