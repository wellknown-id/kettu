# Debugger TODO

- [x] Add first-class opaque resource debugger values so resource constructor locals do not show up as raw integers or generic unknowns.
- [x] Preserve opaque resource summaries through runtime overlays and `evaluate`, keeping Variables and Evaluate aligned on paused frames.
- [x] Add unit and DAP regression coverage for a resource local, including a dedicated debugger fixture.
- [x] Add built-output automation that checks the resource debugger fixture still emits usable DWARF symbols in `kettu build --core --debug` output.
- [x] Add direct debugger coverage for launching from a prebuilt `.wasm` artifact once DAP can consume artifact-only launches without recompiling from source.