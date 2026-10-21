# Destruct-O-Vision

Destruct-O-Vision is a modular video synthesizer that emulates analog video. You patch it like VCV Rack. Instead of applying a VHS look as a filter, it models the signal chain itself: oscillators run in raster time, video passes as composite through a VCR, and a CRT has to lock onto its sync.

**Status: Phase 1 (patchable rack).** The app has a VCV-style rack: modules sit on rails, and you patch them with cables. It ships with 35 modules:
- **Sources:** VCO, Ramps, Shape, Noise, a live Camera (macOS), Text, 3D Text, and 3D Object
- **Processing:** Mixer, VCA, Folder, Keyer, Chroma Key, a line-scan Filter, Blur, Edge, Posterize
- **Color:** Colorize, RGB Combine/Split
- **Space & time:** Feedback frame store, Delay (echo, freeze, and slit-scan through its Time CV), Displace, Mirror, Mosaic
- **Analog signal path:** Encoder, Inject, Cable, VCR, TBC, TV
- **Control:** LFO (Hz, or BPM with note values on a shared beat grid; Trig output), Macros
- **I/O:** Monitor, Output

**3D and text.** 3D Object and 3D Text are raymarched per pixel, so a video signal patched into their rotation, twist or size CVs bends them line by line, and video patched into Tex paints their surfaces. Text renders whatever you type on its panel as a fill, outline, glow or contour bands, in system fonts.

**Patching rules.** Ring colors show what a jack carries: gray and amber are video, blue is control (one value per field), green is composite. Video can plug into blue jacks too; it arrives as the picture's average brightness, a field or two late. Feedback's CVs are video-rate, so a picture patched into them warps the feedback pixel by pixel.

Monitor, Output, Camera and TV show live pictures right in their panels. The Output module can display its picture clean, or on a simulated CRT with scan lines, phosphor mask, glow and curvature.

**The analog signal path (Phase 2) works.** The Encoder turns RGB into a real composite waveform with sync pulses, color burst and modulated chroma. Composite cables (green) carry that waveform through a VCR and a Cable into a TV.

The TV has to lock onto the signal, using a sync separator, a line oscillator with AFC and a field oscillator with V hold. Rolling, tearing, flagging and lost color come from the signal itself. For example, push an oscillator into the sync with Inject and the picture falls apart.

Built-in demo patches:
- `--demo feedback` (the default)
- `--demo analog`
- `--demo time` (delays and chroma key)
- `--demo lab` (3D text over swirling feedback, with video driving a control jack)
- `--demo camera`
- `--demo tv:vcr,cable`, a minimal Encoder → stages → TV chain

**In the rack:**
- **Add a module:** right-click empty rack space.
- **Patch:** drag from one jack to another.
- **Re-patch:** drag a plugged input to pick its cable back up.
- **Disconnect or delete:** right-click a jack or a panel.
- **Probe:** hover a jack to see its signal.
- **Zoom:** Cmd+scroll or pinch.
- **Exact values:** right-click a knob to type one, such as 120 BPM or 3.58 MHz.

Patches are saved to `patches/*.dov` in the folder you started the app from. The packaged app keeps patches and captures in `~/Documents/Destruct-O-Vision`. Undo works with Cmd+Z.

## Download

Get the Mac app from [Releases](https://github.com/bradbrok/destruct-o-vision/releases). It's one app for both Apple silicon and Intel, on macOS 11 or later. Open the DMG and drag Destruct-O-Vision into Applications.

Builds aren't signed by Apple yet, so macOS blocks the app the first time. Open it once, then go to System Settings > Privacy & Security and click **Open Anyway**.

Linux and Windows builds haven't been tried yet. You can [build from source](#build-and-run).

## Writing modules

A module is one WGSL file. A TOML header in its first comment lists the module's ports and params, and a `process` function does the work. The rack draws the panel from the header, and the engine generates everything else: bindings, CV handling and the compute driver. Modules contain no native code, so installing one is safe.

- **Start one:** Modules > New module… writes a starter file into your modules folder. From a terminal, run `destructovision module new yourname.my-effect`.
- **Edit it:** the app reloads a module each time you save it. If a save has an error, the last version that worked keeps running, and the Modules menu shows the error with its line number.
- **Check it:** `destructovision module check FILE|PACKAGE|FOLDER` validates modules without the app. `destructovision module glue FILE` prints the complete shader the engine builds.
- **Examples:** [`examples/modules/example.starter`](examples/modules/example.starter) is a package with a gather, a line-scan kernel and a history-based effect. Every simple built-in is a module file too, in [`crates/dov-modules/modules`](crates/dov-modules/modules).
- **Packages:** a folder with a `package.toml` (id, version, license) and its module files. Drop a package or loose `.wgsl` files into the modules folder: `./modules` when you run from a terminal, or `~/Documents/Destruct-O-Vision/modules` for the app. `--modules DIR` picks another folder.
- **Panel art:** any module's panel can be an SVG named for the module, such as `panels/dov.vco.svg` in the modules folder. That works for built-ins too, and a package of only panels is a skin pack.
  - **Background art:** an SVG with no special markup goes behind the automatic layout and labels.
  - **Full art:** give the SVG a group with id `components` holding one shape per control: `param-<id>` for knobs and switches, `cv-<id>`, `in-<id>`, `out-<id>`, and `text` or `screen` rectangles. The art then sets the panel's width, places every control and draws its own labels, and the `components` group isn't drawn.
  - **Checking art:** `module check` validates art, and art reloads on save like modules do. Modules > Panel art turns it all off.
  - **Example:** [`examples/modules/example.skins`](examples/modules/example.skins) restyles the built-in Folder and Mixer.
- **Sharing patches:** patches record the packages they use. When a patch needs a module you don't have, the rack shows a red "Not installed" stand-in that keeps its settings and cables, and the patch saves back unchanged.

## Build and run

You need Rust through [rustup](https://rustup.rs); `rust-toolchain.toml` pins the version.

```sh
cargo run --release -p dov-app
cargo run --release -p dov-app -- --help
cargo test
```

To build the Mac app and a DMG in `dist/`, run `scripts/package-macos.sh`. Set `UNIVERSAL=1` for Apple silicon and Intel together. The script's header lists the variables for signing and notarizing.

In the output window, **F** toggles fullscreen and **Esc** leaves it. **Snapshot** in the rack window saves the frame and the UI to `captures/`.

## Layout

| Crate | Purpose |
|---|---|
| `dov-core` | Raster standards, phase math, module descriptors, the patch file format and lock-free parameters. Has no GPU dependency. |
| `dov-modules` | The module library: WGSL kernels and CPU behaviors |
| `dov-engine` | The engine thread: patch scheduling, WGSL code generation, GPU runtime, field clock, display flywheel, presentation and capture |
| `dov-app` | The application: the rack UI, the output window, and patch files |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## License

Destruct-O-Vision is free software under the [GNU General Public License v3.0 or later](LICENSE).

The module format is under the [MIT License](LICENSE-MIT), so module authors and other hosts can use it freely. That covers:
- the `dov-format` crate, which parses and checks module files and generates the glue around them, standard WGSL library included
- the built-in module files in `crates/dov-modules/modules`
- the WGSL kernels in `crates/dov-modules/src/library.rs`, which are marked there

Modules you write are yours, under whatever license you choose. They are data the app loads, not part of the app.
