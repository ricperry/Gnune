# Gnune

Gnune (pronounced “noon”) is a Rust implementation of the desktop interface of the original Microsoft Zune Media Player. Recreate as much of its interface as practical using modern, maintainable Rust libraries.

## Project state

Gnune is a Cargo application using GTK4, Lofty, mpv for music, and an embedded GStreamer video and podcast player. Run it with `cargo run`. The README covers runtime dependencies, keyboard shortcuts, and phone pairing. Desktop code lives in `src/` and the installable phone companion lives in `src/mobile/`.

## Implementation approach

- Build in small, runnable increments that reproduce a coherent part of the Zune interface.
- Use reference screenshots, recordings, or documented behavior to guide layout, typography, color, navigation, transitions, and interaction details. State assumptions where references are incomplete.
- Follow the desktop collection at 2:00 and the listening view at 2:40 in the [Zune tutorial](https://www.youtube.com/watch?v=A1iqtznwpCA). Keep the desktop canvas available for typography, library panes and artwork. App-owned windows use content overlays for the three window buttons, which stay visible on a transparent background and support keyboard access.
- Use the author’s latest exported Gnune icon in `assets/` as the source for app icons.
- Inspect existing code and dependencies before choosing libraries. Favor maintained Rust libraries with good Linux support and keep architectural decisions proportional to the implemented scope.
- Target Fedora with GNOME on Wayland. Keep rendering and hardware integration compatible with AMD systems and avoid assumptions that require proprietary GPU tooling.
- Keep changes focused. Preserve unrelated work and document material behavior changes or limitations.

## Verification

- Run `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` where supported by the project. Follow any more specific workspace or package instructions added later.
- Add focused tests for behavior that can be checked reliably, such as navigation state or data transformations.
- For UI changes, launch the application and inspect its rendered output. Compare the affected view and interactions with the available Zune references, including window resizing and relevant navigation states.
- Verify on Fedora GNOME/Wayland when available. Report checks that could not run and distinguish automated or screenshot evidence from human usability assessment.
- Report what changed, how it was verified, and any remaining fidelity gaps. Do not claim completion based only on compilation.
