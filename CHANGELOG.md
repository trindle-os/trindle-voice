# Changelog

## Unreleased

- The speech server moved here from Talk: a priority queue with markers, the speech door a screen reader connects to, the phrase store, speed change, the `pw-play` player and sound cues, as the `speech` module. `trindle-voice serve --dir DIR` runs it and `trindle-voice render-phrases` builds a phrase store. Written-out numbers (`spoken`) and `text::lead_pieces`, which cuts text so the first sound comes early, came with it.
- The code is now under the GNU General Public License, version 3 or later. The original Supertonic MIT notice is kept in NOTICE.
- README laid out with a banner, a three-step picture, settings table and build steps; images and their sources are in `docs/images/`.
- Bug reports go through the Trindle community at https://trindle.buildmorphic.com; SHA256SUMS now lists all ten voice files.

- `speak` now takes the seed itself (`speak_seeded` is gone), a missing voice file reads "the voice files are not all there", the speed message comes from the library only, and `deny.toml` holds the dependencies to GPL-compatible licences.
- Review fixes: the minimum Rust is now 1.91 (what tract needs); `say` no longer follows a link left at the `.wav.part` name; `render-phrases` names the store the way the server looks for it; normal speech now cuts background speech; the README shows the real `key=value` message form.
- Socket activation: `serve` takes the socket systemd passes it (LISTEN_PID, LISTEN_FDS, one Unix socket) and leaves the door file alone when it exits. The package enables the socket for all users. Unit files in `systemd/` and Debian packaging in `debian/`.

## 0.1.0 - 25 October 2026

- First version of the fork, from Supertonic commit 1e9799e (9 September 2026).
- The voice files stay under their own licence, the BigScience Open RAIL-M License; its use restrictions pass on to anyone who uses them. The code was MIT at this release; it is now GPL 3 or later (see Unreleased).
- Runs on tract instead of ONNX Runtime, so the whole program is Rust.
- Text tidying and chunking ported from Supertonic without the regex crate.
- Command line `say` and a small library.
- After code review: speed is limited to 0.25 to 4; long text keeps its order and is cut by characters (Japanese full stops included); empty text and unknown characters are handled; newer `rand`; errors are a small enum; `--seed` for repeatable sound; the command line names mistakes and replaces its output file safely.
