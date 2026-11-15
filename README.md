# hitsmith

A lightweight hitsounding workstation for osu! mappers, written in Rust.

- One 7 MB exe, no runtime to install. Uses Windows' own fonts for Japanese/Chinese/Korean titles.
- No library scan: you open a `.osu` file yourself, so startup is instant.

## Using it

1. Click **Open beatmap...** and pick any difficulty (`.osu`) of the mapset. Its red lines give the beat grid.
2. Name the project and click **Create project**.
3. **+ Track**, choose a sample (from the project, the mapset, or your current osu! skin), then click the grid to place hits.
4. Use the **Volume** lane to add volume points. **Export...** writes a hitsound difficulty into the mapset folder.

| Action | How |
|---|---|
| Place / move / delete a hit | click / drag / right-click on a track |
| Play, pause | Space |
| Back to start | Home |
| Undo, redo | Ctrl+Z, Ctrl+Y |
| Zoom | Ctrl + mouse wheel |
| Scroll | wheel over the waveform, Shift + wheel anywhere, or the bottom slider |

Projects are saved automatically to `projects\` next to the exe (or `%APPDATA%\hitsmith` if that folder isn't writable).

## How export works

Every distinct set of sounds that plays at the same moment is mixed into one `normal-hitnormalN.wav`.
Each circle plays only its hitnormal, and green lines pick the sample index and volume. This reproduces
the arrangement exactly, regardless of skin or layering settings.

Safety rules:

- Sample indices the mapset already uses are skipped.
- An existing difficulty is never overwritten unless this app created it.
- Files from a previous export that are no longer needed are removed.
- What the app created is tracked in `.hitsmith-export.json` inside the mapset folder.

## Building

Needs Rust (`rustup`, toolchain `stable-x86_64-pc-windows-gnu`) and [WinLibs MinGW-w64](https://winlibs.com/)
extracted to `%USERPROFILE%\winlibs`. Then run:

```
build.cmd
cargo test
```

(`cargo test` needs the same PATH as `build.cmd` sets.)
