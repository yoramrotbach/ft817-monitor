# Developer notes

Building, testing and releasing 817 Monitor and 817 EEPROM. User documentation is in the [README](../README.md).

## Build from source

Requirements: [Rust](https://rustup.rs) (stable), Node.js 18+, and on Linux:
`sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libudev-dev`

```
npm install
npm run demo          # 817 Monitor with the simulated radio
npm run dev           # 817 Monitor
npm run build         # installers in target/release/bundle/
npm test              # all unit + end-to-end tests (cargo test --workspace)

cd eeprom && npm install
npm run dev           # 817 EEPROM (choose "Demo" to use the simulated radio)
npm run build
```

## Testing without a radio

```
# Linux/macOS: a virtual serial cable with a simulated FT-817 on one end
socat -d -d pty,raw,echo=0,link=/tmp/radio pty,raw,echo=0,link=/tmp/app
cargo run -p ft817-monitor --example sim_serial -- /tmp/radio            # add --static for EEPROM tests
# then select /tmp/app in either app, or from the command line:
cargo run -p ft817-monitor --example cli_monitor -- serial /tmp/app 38400
cargo run -p ft817-eeprom  --example ee_cli -- backup /tmp/app 38400 test.817ee
```

On Windows, use a virtual COM-port pair such as com0com.

## Releasing

Either push a version tag (`git tag v0.1.0 && git push --tags`), or open the Actions tab → release → "Run workflow" and enter a version such as `v0.1.1`. GitHub Actions builds every platform and creates a draft release with the installers attached.

## Command-line EEPROM tool

Command line, same engine: `cargo run -p ft817-eeprom --example ee_cli -- backup <port> [baud] out.817ee` and `... restore <port> [baud] file.817ee --yes`.

## Project layout

```
crates/ft817-cat/           shared, read-only: CAT frames and decoders, serial link, simulated FT-817 (with EEPROM)
src-tauri/, ui/             817 Monitor (polling loops, rigctld, display)
eeprom/src-tauri/src/engine.rs      817 EEPROM: backup, restore plan, guarded writes (the only write code)
eeprom/src-tauri/src/backupfile.rs  the .817ee file format
eeprom/ui/                  817 EEPROM window
docs/PROTOCOL.md            what is read and written, how it's decoded, and the open questions
```
