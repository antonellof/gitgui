You are building `gitgui`, a native git GUI rendered as pixels inside kitty-graphics terminals. Read `CLAUDE.md`, `docs/SPEC.md`, and `docs/PROTOCOLS.md` in full before doing anything else. They define the architecture, protocols, module layout, tests, and milestones. Treat them as the source of truth.

Context you should know:

- This is the same trick as zenbu-labs/terminal-browser (pixels via the kitty graphics protocol, synthetic input from terminal mouse and keyboard events) but with no Chromium. We render an egui UI with our own software rasterizer and talk to git through libgit2.
- I am running you inside `cmux` on `macOS`. That is the primary target. Use it for every manual check.
- Rust stable is installed. Check `rustc --version` and `cargo --version` first and pin the newest egui and git2 that compile on it. Record the pinned egui version and any API notes in CLAUDE.md under a new "Pinned versions" section.

How to work:

1. Start with Phase 0 from SPEC section 8. Do not scaffold later phases up front; create modules when their phase starts.
2. Before writing `term/kitty.rs` and `term/input.rs`, write their unit tests from the byte sequences in PROTOCOLS.md. Then implement until green.
3. At the end of each phase: `cargo test`, `cargo clippy -- -D warnings`, `cargo run -- --headless-frame /tmp/tg.png` (from Phase 1 on) and look at the PNG, then run the manual check listed for that phase and ask me to confirm what I see. Commit with a message that names the phase. Only then move to the next phase.
4. If a protocol detail in PROTOCOLS.md does not match what the terminal actually does, verify with `--dump-input` or `--probe`, fix the doc in the same commit, and tell me what changed.
5. For split integration (Phase 5) inspect the real cmux CLI on this machine before implementing. Do not invent flag names.
6. Keep `main.rs` small, keep the rasterizer as the only hot path, and profile it with `cargo build --release` and a 1600x1000 frame before adding any optimization.
7. Never use the em dash character in code, comments, docs, or commit messages.

Report format after each phase: what was built, test count, frame time for a 1600x1000 render in release, open issues, and the exact manual check you want me to run.

Begin with Phase 0 now.
