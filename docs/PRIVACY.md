# Privacy Policy

Vaporly is a dictation app that runs entirely on your Mac. This policy explains,
in plain language, what it touches and what it does not. The short version: your
voice and your words stay on your computer.

## What Vaporly runs on

Everything happens locally on your machine. There is no Vaporly account, no
sign-in, and no cloud service processing your speech. The app turns your voice
into text using AI models that run on your own Mac.

## What Vaporly accesses

- Your microphone, but only while you are actively dictating. When you are not
  dictating, the app is not listening.
- The text it produces from your speech, so it can clean it up and hand it to
  you.
- Your clipboard, so it can paste the finished text into whatever app you are
  using.
- The name of the app you are currently in (for example "Mail" or "Notes"), so
  it can format the text sensibly for that app. It reads the name only, never
  the contents of that app.

## What Vaporly stores, and where

Vaporly keeps a few things on your Mac so the app stays useful across sessions:

- Audio recordings of your dictations (WAV files).
- Your dictation history (a local database).
- Your settings, custom words, and snippets.

All of this lives in your user Library folder, on your machine only:

    ~/Library/Application Support/computer.vaporly/

Nothing in that folder is uploaded anywhere. It is as private as any other file
on your computer.

## What reaches the internet

Only two things ever leave your Mac, and neither one sends any personal
information about you:

1. App updates. Vaporly checks GitHub to see whether a newer version is
   available, and downloads the update if you choose to install it.
2. AI models. The first time you need the speech model or a cleanup model,
   Vaporly downloads it from Hugging Face. This is a one-time download of the
   model file itself.

That is the entire list. Your audio, your transcripts, and your history are
never part of these connections.

## What Vaporly does NOT do

- No telemetry.
- No analytics or usage tracking.
- No crash reporting back to us.
- No account and no login.
- No cloud AI. The AI runs on your Mac.

## Deleting your data

You are always in control of your data:

- Clear your dictation history from inside the app at any time.
- To remove everything, delete the app-data folder listed above
  (`~/Library/Application Support/computer.vaporly/`), then delete the app
  itself.

Once those are gone, nothing of yours remains.

## Children

Vaporly is a general-purpose tool and is not directed at children. It does not
knowingly collect information from anyone, of any age, because it does not
collect information about you at all.

## Changes to this policy

This policy may change at any time, without notice. The version published in
the app's repository is always the current one, and because Vaporly does not
have your contact information, the repository is the place to check. Continuing
to use Vaporly means you accept the current version.
