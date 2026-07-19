# Whisper Mode

Whisper mode lets you dictate **quietly**, for example in a shared office, a library, or a room where someone is asleep, while Vaporly ignores louder sounds around you. You speak softly; a normal talking voice, a TV, or a nearby conversation is treated as background and left out of your transcript.

Turn it on under **Settings > Dictation > Whisper mode**, pick a strength, and optionally bind a key to toggle it quickly.

## What it does, in plain terms

Three things happen when whisper mode is on:

1. **Your quiet voice gets a boost** so the speech model can still understand it. Vaporly amplifies soft input before it reaches the recognizer.
2. **Loud sound gets gated out.** Vaporly measures how loud the incoming sound is and, when it stays above a cutoff, treats it as background noise and drops it from the recording. So a whisper is captured; a full voice or a blaring TV is ignored.
3. **Voices and far-away sound get extra checks.** Clearly reverberant sound (the mark of a source far from your microphone) is rejected at every strength, and on High, sound with a vocal pitch is rejected too: a true whisper has no pitch, so a talking voice is treated as background no matter how quietly it reaches the mic.

The result: you can murmur a sentence and get clean text, while the loud world around you does not end up in your dictation.

## Strengths

The strength sets both how hard your voice is boosted and how quiet you intend to be (the loudness cutoff):

- **Light**: ignores only genuinely loud sound. A normal speaking voice still gets through. Gentlest boost. Good when the room is mostly quiet and you just want to keep out bangs and music.
- **Medium** (default): ignores a normal talking voice, captures soft speech. This is the everyday "talk quietly" setting.
- **High**: expects a true whisper and boosts hardest. Only very quiet input is captured; anything at conversational volume is treated as background.

If your captured text is choppy, you are probably speaking louder than the strength expects, or too quietly for it; move one step toward Light (to capture more) or High (to reject more) and try again.

## Calibrate for your microphone

The built-in cutoffs are sensible averages, but every microphone and every room is different. **Settings > Dictation > Mic calibration** runs a short wizard that measures three things on your actual setup: the room's background level, your normal voice, and your whisper. From those it tunes the whisper cutoffs to your hardware and tells you how cleanly your whisper separates from your voice.

You set the pace: each step records until you press the button (it unlocks after 5 seconds), so take your time and speak naturally. The whole thing takes about half a minute.

**The room is part of the measurement.** A calibration made at your desk describes that desk: its microphone, its background hum, its echo. Run the wizard again whenever you dictate somewhere new: a different room, a different mic or headset, or a noticeable change in background noise. Calibrations are stored per microphone, and Vaporly falls back to the built-in cutoffs whenever the active mic does not match the one you calibrated.

## How the gate works (for the curious)

Under the hood the gate watches the loudness of the incoming sound over time, not instant by instant, so it decides per **burst** of speech rather than clipping individual syllables. It tracks two smoothed loudness envelopes:

- A **slow** envelope that rides over the natural gaps between words. When it stays above the strength's cutoff, the gate **closes** (this is what rejects a sustained loud voice, even though speech has short pauses).
- A **fast** envelope that reacts quickly to real quiet. When it drops and stays low, the gate **reopens**.

Using two envelopes with opposite reaction speeds is what lets Vaporly reject a loud talking person (whose voice keeps dipping into word-gaps) while still reopening promptly when you actually go quiet again. The loudness cutoffs are roughly Light 0.08, Medium 0.03, High 0.012 on the raw microphone level; these were calibrated against real recordings.

## Tips

- Run **Mic calibration** first; it beats guessing at strengths. And recalibrate whenever you change rooms, mics, or the background noise changes; the room is part of the measurement.
- Whisper mode is tuned on a per-machine basis. If Medium is not quite right for your microphone, try Light or High first; they shift the cutoff by a meaningful amount.
- Hold the microphone or your Mac a bit closer when whispering on High; the boost is large but a very distant whisper can still fall below the voice detector.
- Whisper mode only changes capture. All your other settings (cleanup dials, custom words, overlay) work exactly the same.

## Troubleshooting

- **My whisper produces nothing.** The strength may be rejecting you as "not quiet enough" in the wrong direction, or the input is below the voice detector. Try Medium, speak a touch louder, and move closer to the mic.
- **Loud sound still gets through.** Move up a strength (Light to Medium, or Medium to High). There is also a brief moment at the very start of a loud sound before the gate closes; sustained loud sound is rejected, a split-second onset may slip in.
- **It clips the middle of my words.** Move one step toward Light. Your speech is dipping below the cutoff between syllables at the current strength.
