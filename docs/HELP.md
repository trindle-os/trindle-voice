# Trindle Voice help

## What it does

Trindle Voice turns text you type into spoken words and saves them as a sound file (WAV). It makes the sound on your own computer. It has ten voices, M1 to M5 and F1 to F5, and speaks 31 languages.

## Before you start

The voice files are not included, so you download them once from [Hugging Face](https://huggingface.co/Supertone/supertonic-2) into one folder. You need these four model files, the indexer file and the voice files you want:

```
supertonic-duration_predictor.onnx   supertonic-text_encoder.onnx
supertonic-vector_estimator.onnx     supertonic-vocoder.onnx
supertonic-unicode_indexer.json      supertonic-voice-M1.json  (and the other voices)
```

Check the downloads against [SHA256SUMS](../SHA256SUMS) by running `sha256sum -c SHA256SUMS` inside that folder.

## How to start it

Build it once, then run it with your text at the end:

```bash
cargo build --release
./target/release/trindle-voice say --dir ~/voice-files --voice F1 --out hello.wav "Hello from Trindle."
```

Open `hello.wav` in any player. If `hello.wav` already exists it is replaced. The text has to be typed after the options. It cannot be piped in.

## Every setting

| Option | Needed | Default | What it does |
|---|---|---|---|
| `--dir` | Yes | none | The folder that holds the voice files |
| `--out` | Yes | none | The sound file to write |
| `--voice` | No | `M1` | M1 to M5 or F1 to F5 |
| `--lang` | No | `en` | The language of the text (see below) |
| `--speed` | No | `1.05` | 0.25 to 4. Higher is faster, 1 is the voice's own pace |
| `--seed` | No | random | A whole number. The same text, voice, speed and seed give the same sound. Without it every run sounds a little different |

Languages: en, ko, ja, ar, bg, cs, da, de, el, es, et, fi, fr, hi, hr, hu, id, it, lt, lv, nl, pl, pt, ro, ru, sk, sl, sv, tr, uk, vi. The tag `na` is for text with no language.

Long text is cut into pieces at paragraphs, sentences and commas, and joined with a short pause between them. The loudness is evened out before the file is written. Emoji and some symbols are dropped from the text before it is spoken.

## Does it work offline?

Yes. The only time you need the internet is to download the voice files. After that it needs no connection at all.

## What can go wrong

| What you see | What it means | What to do |
|---|---|---|
| `failed: the voice files are not all there` | The folder is missing, or a model file, the indexer or the voice you asked for is not in it | Check the `--dir` path and that all the files are there with the exact names above. Check the voice name is M1 to M5 or F1 to F5 and that you downloaded that voice |
| `failed: --dir is required` or `--out is required` | You left out a needed option | Add it |
| `failed: unknown option --xyz` | A mistyped option | Check the spelling against the table above |
| `failed: speed must be between 0.25 and 4` | The speed is outside the range | Choose a number from 0.25 to 4 |
| `failed: --speed needs a number` or `--seed needs a whole number` | Something other than a number was typed | Use digits only. The seed has no decimal point |
| `failed: nothing to say` | The text is empty, or only emoji or spaces | Give it some words |
| `failed: unknown language: xx` | The language code is not one of the list above | Pick a code from the list |
| `failed: bad voice name` | The voice name has a space or a symbol in it | Use letters and numbers only, for example F2 |
| `failed:` followed by a file path and a reason | The sound file cannot be written, for example the folder does not exist | Make the folder first, or choose another place |
| `failed: the model gave a length that makes no sense` | The model returned a length that cannot be right, which can mean damaged voice files | Download the files again and check them against SHA256SUMS |
| `failed: too much audio for one WAV file` | The text is so long that the sound would not fit in one WAV file | Split the text and make several files |
| It sounds odd on one run | Each run without a seed is a little different | Run it again, or fix a seed you like |

## Your privacy

- **What it hears:** nothing. It does not use a microphone. It only reads the text you type.
- **What it keeps:** only the sound file you ask for, in the place you name. It keeps no log and no copy of your text.
- **What it sends:** nothing. It makes no network connection, so your text never leaves your computer.

## More

[README](../README.md), [SECURITY](../SECURITY.md) for private reports, and bug reports through the Trindle community at https://trindle.buildmorphic.com.
