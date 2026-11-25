# 817 Monitor

A desktop CAT monitor for the **Yaesu FT-817, FT-817ND and FT-818**. It shows what the radio is doing on a large, LCD-style panel: frequency, mode, S-meter, transmit meters and front-panel settings. The idea comes from hardware CAT displays for the 817, but this is an independent design for Windows, macOS and Linux (including Raspberry Pi).

**Read-only by design.** The app never changes anything on the radio. Every frame it sends passes through an allow-list of five read commands, so it cannot send a write, a mode change or the factory reset.

![Receive](docs/screenshot-rx.png)
![Transmit](docs/screenshot-tx.png)

## What it shows

| Area | Receive | Transmit |
|---|---|---|
| Header | VFO A/B, MEM, HOME, QMB · band · mode (incl. DIG sub-mode, narrow filters) · BUSY · RX | TX |
| Frequency | 10 Hz resolution, radio-style `14.074.00` | same |
| Meter | S-meter S1–S9+60 with peak hold | PO, plus SWR / ALC / MOD (optional, see below) |
| Flags | AGC, NB, DSP, PBT, LOCK, FST, SPLIT, VOX, BK, KYR, antenna front/rear, power level | + HI-SWR alert |
| Alerts | CTCSS/DCS mismatch (TONE?), FM off-frequency | |

The LCD color follows the radio's own color setting (menu "Color": blue or amber), or you can pick amber, blue or a dark night mode.

## Connecting

**Direct serial (full detail).** Connect the radio's ACC jack to the computer with a CAT cable (CT-62 type or similar) or an interface such as the Digirig. In Settings, choose the port and the baud rate that matches the radio's CAT RATE menu setting (4800, 9600 or 38400). The link is fixed at 8 data bits, no parity, 2 stop bits.

- Only one program can use the CAT port at a time. A hardware CAT display on the same ACC jack, WSJT-X or a logger will conflict with it.
- RTS and DTR are set low right after the port opens, because some interfaces key the transmitter on them.
- The app polls about 40 cycles per second (around 100–120 frames per second), similar to established hardware displays.

**Hamlib rigctld (share the radio).** To run the monitor alongside WSJT-X or a logger, let rigctld own the port and point everything at it:

```
rigctld -m 1020 -r COM3 -s 38400      # FT-817 / FT-817ND
rigctld -m 1041 -r /dev/ttyUSB0 -s 38400   # FT-818
```

Then choose "Hamlib rigctld" in Settings (default 127.0.0.1:4532) and set WSJT-X to "Hamlib NET rigctl". This mode shows frequency, mode, VFO, split, S-meter, PTT and approximate TX meters. Panel flags (AGC, NB and so on) aren't available through Hamlib.

**Demo.** A built-in simulated FT-817 that cycles through FT8 on 20 m, CW on 40 m and FM on 2 m. You can also start the app with the `--demo` argument (on Linux: `ft817-monitor --demo`).

## Transmit meters (SWR, ALC, MOD)

Power output always works. SWR, ALC and MOD need the undocumented command `0xBD`, so they are **off by default**. Turn them on in Settings → Transmit meters.

Why it's optional: `0xBE`, one value away, is the factory reset, which erases the radio's calibration. The app cannot send `0xBE` (it is blocked in code and covered by a unit test), but a noisy cable could in theory corrupt a byte in transit. With a decent cable the risk is very low, and hardware displays have used `0xBD` for years.

Published sources disagree on how the `0xBD` reply is packed. If SWR moves with your voice and ALC follows the antenna, switch "Reply layout" in Settings.

## Install

Download the installer for your system from the [Releases](../../releases) page:

| System | File |
|---|---|
| Windows 10/11 | `.msi` or `-setup.exe` |
| macOS (Apple Silicon / Intel) | `.dmg` (`aarch64` / `x64`) |
| Linux x64 | `.deb`, `.rpm` or `.AppImage` |
| Raspberry Pi 4/5 (64-bit Raspberry Pi OS) | `arm64` `.deb` |

The builds are not code-signed yet, so you'll see a warning the first time:

- **Windows:** SmartScreen → "More info" → "Run anyway".
- **macOS:** right-click the app → Open → Open. If macOS reports the app as damaged, run `xattr -cr "/Applications/817 Monitor.app"`.
- **Linux:** add yourself to the serial group to open the port without root: `sudo usermod -aG dialout $USER` (log out and back in).

## Build from source

Requirements: [Rust](https://rustup.rs) (stable), Node.js 18+, and on Linux:
`sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libudev-dev`

```
npm install
npm run demo          # run with the simulated radio
npm run dev           # run normally
npm run build         # installers in src-tauri/target/release/bundle/
npm test              # unit + end-to-end tests
```

### Testing without a radio

```
# Linux/macOS: a virtual serial cable with a simulated FT-817 on one end
socat -d -d pty,raw,echo=0,link=/tmp/radio pty,raw,echo=0,link=/tmp/app
cargo run --manifest-path src-tauri/Cargo.toml --example sim_serial -- /tmp/radio
# then select /tmp/app in the app, or watch the decoded state as JSON:
cargo run --manifest-path src-tauri/Cargo.toml --example cli_monitor -- serial /tmp/app 38400
```

On Windows, use a virtual COM-port pair such as com0com.

### Releasing

Either push a version tag (`git tag v0.1.0 && git push --tags`), or open the Actions tab → release → "Run workflow" and enter a version such as `v0.1.1`. GitHub Actions builds every platform and creates a draft release with the installers attached.

## Project layout

```
src-tauri/src/cat.rs        protocol: read-only frame builder and decoders (unit tested)
src-tauri/src/poller.rs     polling loops for serial/demo and rigctld, reconnect logic
src-tauri/src/transport.rs  serial port and simulator links
src-tauri/src/sim.rs        simulated FT-817
ui/                         the display (plain HTML/CSS/JS, no build step)
docs/PROTOCOL.md            what is read, how it's decoded, and the open questions
```

## Credits

Protocol knowledge comes from Clint Turner KA7OEI's FT-817 CAT and memory-map pages and from the Hamlib FT-817 backend. See [docs/PROTOCOL.md](docs/PROTOCOL.md).

Not affiliated with Yaesu, or with any maker of CAT display hardware. Yaesu, FT-817 and FT-818 are trademarks of their owners.

## License

MIT, by 4Z1YR. Contributions and test reports from FT-817 and FT-818 owners are welcome. Please include your radio model and firmware when reporting meter or flag issues.
