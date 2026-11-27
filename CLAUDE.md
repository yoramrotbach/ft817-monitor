# CLAUDE.md

Desktop apps for the Yaesu FT-817 family: **817 Monitor** (read-only CAT display) and **817 EEPROM** (EEPROM backup and restore). Owner: Yoram, 4Z1YR. v0.1.0 is released.

## Read first

- `docs/HANDOFF.md`: project status, test coverage, decisions log, open items.
- `docs/DEVELOPING.md`: building, testing without a radio, releasing.
- `docs/PROTOCOL.md`: the CAT protocol and the real FT-817ND findings that override published sources.

## How to work with Yoram

- **Plan first.** Present a change plan and wait for explicit approval before changing code or starting a build.
- **Give a professional opinion**, even when it disagrees with his. Don't invent facts: say "I don't know" or "I couldn't find" instead.
- **Real hardware decides.** When the radio contradicts documentation, follow the radio and record it in `docs/PROTOCOL.md`.
- **US English** in all code comments, UI text and documents.
- **README.md is for users only.** Developer and release instructions go in `docs/DEVELOPING.md`.
- **Builds go into a draft release.** Nothing is published until Yoram has tested it. A new release needs a new version number in all six version fields (see `docs/HANDOFF.md`).

## Safety rules (never weaken)

- 817 Monitor only sends read commands, through the allow-list in `crates/ft817-cat/src/cat.rs`. `0xBE` (factory reset) must stay impossible to build; a test enforces it.
- Only `eeprom/src-tauri/src/engine.rs` writes to the radio: never below 0x0006, every write verified, a fresh backup saved first, and the wrong-radio check before a restore.
- Keep RTS and DTR low on the serial port; interfaces like the Digirig key PTT on RTS.

## Commands

- Tests: `cargo test --workspace` (all must pass).
- Release build: `gh workflow run release.yml -R yoramrotbach/ft817-monitor --ref main -f version=vX.Y.Z`
- Commits: author `Yoram (4Z1YR) <yoramrotbach@users.noreply.github.com>`.
