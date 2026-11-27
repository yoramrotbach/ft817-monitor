# 817 Monitor & 817 EEPROM: Project Handoff

As of Nov 27, 2026 · Yoram (4Z1YR). The same handoff is also kept as a Claude doc: <https://claude.ai/artifact/LPfD6yrL35YS4PnzZnLkHF>

## Summary

Version 0.1.0 of both apps was released publicly on GitHub on Nov 27, 2026, and the project is closed for now. Both apps were verified on real hardware with two FT-817ND radios.

- **817 Monitor** is a large, resizable desktop display of what the radio is doing, a look-alike of the classic Arduino CAT display boxes. It only reads from the radio.
- **817 EEPROM** backs up and restores the radio's EEPROM (memories, settings and the 76 soft-calibration values) with verified writes and several safeguards.
- Both run on Windows, macOS (Apple silicon and Intel), Linux PCs and the Raspberry Pi (64-bit).
- The project is personal and open source (public GitHub repository), with an experimental-use disclaimer.

| What | Where |
| --- | --- |
| Repository | [github.com/yoramrotbach/ft817-monitor](https://github.com/yoramrotbach/ft817-monitor) |
| Release v0.1.0 (24 installers) | [Releases page, tag v0.1.0](https://github.com/yoramrotbach/ft817-monitor/releases/tag/v0.1.0) |
| User guide | README.md in the repository root |
| Developer guide | docs/DEVELOPING.md |
| Protocol notes and field findings | docs/PROTOCOL.md |
| Release commit | 7e44b80 on main |

## Repository, builds and releases

All builds run on GitHub Actions, free for this public repository; nothing is built on your PC. A release is started by hand and lands as a draft that you publish.

| Path | Contents |
| --- | --- |
| `crates/ft817-cat/` | Shared Rust library: CAT frames, decoders, serial transport, simulated radio |
| `src-tauri/` + `ui/` | 817 Monitor: Rust back end and the HTML/CSS/JS display and Settings window |
| `eeprom/src-tauri/` + `eeprom/ui/` | 817 EEPROM: back end (the only code that writes to the radio) and its window |
| `.github/workflows/release.yml` | Builds both apps for all platforms into a draft release |
| `.github/workflows/ci.yml` | Runs the tests on every push |
| `docs/` | DEVELOPING.md, PROTOCOL.md, this handoff, screenshots |

**To make a new release:**

1. Raise the version (for example to 0.1.1) in all six places: `package.json`, `eeprom/package.json`, `src-tauri/tauri.conf.json`, `eeprom/src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `eeprom/src-tauri/Cargo.toml` (and `crates/ft817-cat/Cargo.toml` if the library changed).
2. Push to `main`, then start the workflow: `gh workflow run release.yml -R yoramrotbach/ft817-monitor --ref main -f version=v0.1.1` (or Actions → Release → Run workflow, with the version).
3. The run takes about 10–15 minutes and builds 24 installers (12 per app) into a draft release.
4. On GitHub, open the draft, set the title and notes, check that Target is `main`, and click Publish. Publishing creates the tag.

**Constraints to know:**

- Claude sessions on claude.ai can push code and start workflows, but can't push tags or create or edit releases through the GitHub API. Publishing v0.1.0 was done in the browser, signed in as Yoram.
- macOS builds are ad-hoc signed, not notarized: users need "Open Anyway", and `xattr -cr` if macOS says the app is damaged. Notarizing would need a paid Apple Developer account.
- Windows builds are unsigned, so SmartScreen warns once ("More info" → "Run anyway").
- v0.1.0 is published and fixed; any change needs a new version number.

## Architecture

Two Tauri v2 desktop apps (Rust back end, plain HTML/CSS/JS front end) share one Rust library for the CAT protocol, and only one of them can write to the radio.

```
 817 Monitor ──── reads only ─────►┐
                                   │  ft817-cat library  ◄── CAT ──►  FT-817ND
 817 EEPROM ◄─── reads + writes ──►┘  (frames, decoders,              (USB serial, e.g. Digirig,
 (the only code that writes)           serial, simulated radio)         RTS/DTR held low)
```

- **817 Monitor** polls frequency/mode, RX or TX status and, while transmitting, the meters; panel flags come from EEPROM reads about every 0.7 s. Connection is direct serial or Demo; Hamlib was removed.
- **817 EEPROM** reads 0x0000–0x1925 twice and reconciles differences; a restore writes only differing bytes, verifies each, and keeps bytes 0x00–0x05 and the radio's CAT rate. Backups are `.817ee` JSON files with a SHA-256 checksum, saved automatically in the app's data folder.
- **The simulated radio** behaves like a real FT-817 down to wiping itself if the header is written, so the safeguards are tested without a radio.

## FT-817ND findings that differ from published sources

Testing on two FT-817NDs ("Lower" and "Upper", both unmodified) contradicted the published documentation in six places; the apps follow the radios. All are recorded in docs/PROTOCOL.md.

| Item | Published sources say | Real FT-817ND does | How the apps handle it |
| --- | --- | --- | --- |
| TX meter reply `0xBD` | KA7OEI: PWR\|SWR, MOD\|ALC | PWR\|ALC, SWR\|MOD, checked against the radio's own meters | Fixed layout; the layout setting was removed |
| PKT mode code (`0x03` reply) | `0x0C` | `0xFC` | Both codes shown as PKT; unknown codes never get "-N" |
| Dial LOCK (EEPROM 0x57 bit 6) | 1 = locked | 0 = locked | Reversed in both apps; verified on both radios |
| Header bytes 0x00–0x03 | A checksum; changing it can wipe the EEPROM | Fixed `A5 5A A5 5A` on both radios | Never written either way |
| EEPROM read past 0x1925 | Returns a single `00` | Returned something else (first radio) | Backup reads exactly 0x0000–0x1925, never past the end |
| CAT while LOCK is on | Radio may stop answering | Answered normally (backups and restores worked locked) | Lock check kept as a precaution |

**Radio facts worth keeping:**

- Version bytes (0x04–0x05): Lower `CE BF`, Upper `D8 BF`.
- 64 of the 76 calibration values differ between the two radios. Factory calibration varies widely: Lower has transmit gain 255 on 6 m and UHF, Upper has much higher HF power settings; both are factory values.
- The radios' band memories start below the ham bands (30 m slot at 10.000 MHz, 17 m at 18.000 MHz), so the Monitor shows GEN there. That was reviewed and left as is.
- TONE? and OFF-FREQ (E7 bits 6 and 5) were set in SSB and at key-up, so they're shown only in FM, FM-N and PKT receive. Their FM behavior hasn't been checked against a real off-tone or off-frequency signal.

## Test status

Windows is fully tested on real hardware; macOS hasn't been tested with a radio at all. All 28 automated tests pass on every build.

| Area | Windows | Linux (Ubuntu VM) | Raspberry Pi | macOS |
| --- | --- | --- | --- | --- |
| Monitor, receive display | Tested | Tested | Tested | Not tested |
| Monitor, TX meters vs radio | Tested | Not tested | Not tested | Not tested |
| Monitor, PKT and narrow | Tested | Not reported | Not reported | Not tested |
| Monitor, zoom | Tested | Tested | Not reported | Not tested |
| Monitor, Connect / Disconnect | Tested | Tested after fix | Not reported | Not tested |
| EEPROM backup | Tested, both radios | Not reported | Tested | Not tested |
| EEPROM restore and wrong-radio warning | Tested | Not tested | Not tested | Not tested |

The final LOCK fix (both apps) and the TONE?/OFF-FREQ fix were confirmed on real hardware; which computer and radio were used wasn't recorded. Full screen and the remembered window size weren't reported on any system.

**Automated tests (28, run by CI on each push):** CAT decoding, including the measured meter, PKT and LOCK values; the read-only command allow-list, including a test that the factory-reset command `0xBE` can't be built; EEPROM backup with dropped and corrupted replies; restore planning and verified writes; protection of 0x00–0x05; and the wrong-radio check using the two radios' real calibration values.

The Monitor build being tested before release also ran end to end against the simulated radio over a virtual serial port. A Linux click test confirmed the Disconnect fix (30 of 30 clicks, against 1 of 30 before).

## Decisions log

The decisions below shape the current design; each was made on test evidence, newest first.

| Date | Decision | Why |
| --- | --- | --- |
| Nov 27, 2026 | Reverse the dial LOCK bit in both apps | Both radios showed the bit is 0 when locked |
| Nov 27, 2026 | Show TONE? and OFF-FREQ only in FM, FM-N and PKT receive | They appeared falsely in 2 m USB transmit |
| Nov 27, 2026 | Write screen text only when it changes | WebKit (Linux, macOS) dropped Disconnect clicks while the text was rewritten 25 times a second |
| Nov 27, 2026 | Zoom the whole LCD with the window; full screen; remember window size | The radio's LCD is too small to read comfortably; the PC display is the point of the project |
| Nov 27, 2026 | Leave GEN outside the ham band edges | Correct band names were confirmed inside the bands |
| Nov 26, 2026 | Remove Hamlib rigctld support entirely | Hamlib 4.7.2 couldn't show PKT or FM-N and stalled about 27 s when the radio sent its 1-byte meter reply |
| Nov 26, 2026 | Fixed TX meter layout PWR\|ALC, SWR\|MOD; remove the layout setting | Matched the radio's own meters; the other published layouts didn't |
| Nov 26, 2026 | Warn on restore when more than 8 calibration values differ; require typing DIFFERENT RADIO | The two radios differ in 64 of 76 values; a radio's own backup differs in about 0 |
| Nov 26, 2026 | Back up exactly 0x0000–0x1925, never probe past the end | The first real radio broke the end probe |
| By Nov 25, 2026 | 817 EEPROM as a separate app with restore; never write 0x00–0x05; keep the radio's CAT rate | Keeps the Monitor read-only; a backup without restore was pointless |
| By Nov 25, 2026 | Ad-hoc sign the macOS apps | Fixed the "app is damaged" error |
| By Nov 25, 2026 | Separate Settings window; connection error stays on screen with a 5 s countdown | Stretching one window stretched the other; the error flashed too fast to read |
| By Nov 25, 2026 | Tauri v2 + Rust, one shared CAT library, public GitHub, builds on GitHub Actions | Small native installers for all four platforms, free builds |

Option B, letting the Monitor itself share the radio with WSJT-X, was considered and deferred; the Monitor currently needs the CAT port to itself.

## Open items and next steps

Nothing is blocking; these are the known gaps, most useful first.

- [ ] **Test on macOS with a radio:** Monitor display, Disconnect, full screen (F11 or Control-Command-F), and an EEPROM backup.
- [ ] **Check full screen and the remembered window size** on Windows and Linux; neither could be tested in Claude's screen-less environment.
- [ ] **Check TONE? and OFF-FREQ in FM:** TSQ set to a wrong tone with a station keying (TONE? should light); a steady FM signal tuned a few kHz off (OFF-FREQ should light). The bit meanings come from published docs, not the radios.
- [ ] **TX meters on Linux and Raspberry Pi:** compare against the radio's own meters, as was done on Windows.
- [ ] **Decide on the lock check for backups:** the radios answered normally while locked, so the check could be dropped for backups (read-only) and kept for restores.
- [ ] **Option B (deferred):** the Monitor shares the radio with WSJT-X or a logger by acting as a Hamlib-compatible network server, forwarding only the commands those programs need, off by default. A larger job that would end the Monitor's read-only design.
- [ ] **Not tested at all:** the original FT-817 and the FT-818. If anyone reports results, the first things to check are the meter layout, the PKT code and the LOCK bit.

## Working agreements and how to resume

To pick the project up again in Claude Code, open this repository and start with: "Read CLAUDE.md and docs/HANDOFF.md, then wait for my instructions." CLAUDE.md is loaded automatically and points here.

**Ways of working that held up during the project:**

- **Plan first, then approval:** Claude presents a change plan and waits for Yoram's approval before changing code or starting a build.
- **Real hardware decides:** where the radio contradicts published documentation, the radio wins, and the finding goes into PROTOCOL.md.
- **Evidence before fixes:** a screenshot, the radio's own meter, a backup file or a log (like the rigctld log) settles a cause before code changes.
- **Builds go to a draft release; nothing is published until Yoram has tested it.**
- **The README is for users only.** Build, test and release instructions live in docs/DEVELOPING.md.
- **US English** in all text and documents.

**Safety rules built into the code (keep them):**

- 817 Monitor sends only read commands, through an allow-list; the factory-reset command `0xBE` can't be built, and a test enforces it.
- Only 817 EEPROM writes to the radio, never below address 0x0006, with every write verified and a fresh backup saved first.
- RTS and DTR are held low on the serial port, so interfaces like the Digirig never key the transmitter.

**Commit conventions:** author `Yoram (4Z1YR) <yoramrotbach@users.noreply.github.com>`; Claude's commits end with Co-Authored-By and Claude-Session lines.
