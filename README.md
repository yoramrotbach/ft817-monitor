# FT-817 tools: 817 Monitor and 817 EEPROM

> **⚠️ Experimental software, made for personal use.** Use it at your own risk. The author accepts no responsibility for any damage to your radio, including loss of calibration, settings or memories. See [Disclaimer](#disclaimer).

Two free desktop apps for the **Yaesu FT-817, FT-817ND and FT-818**, for Windows, macOS and Linux (including Raspberry Pi):

- **[817 Monitor](#817-monitor)**: a large LCD-style display of what the radio is doing. Strictly read-only.
- **[817 EEPROM](#817-eeprom)**: back up the radio's whole EEPROM (calibration, settings, memories) and restore it with verified writes.

They are separate apps on purpose: the monitor contains no code that can write to the radio.

> **Tested radios.** Both apps have been tested on real hardware with two **FT-817ND** radios only. They have **not** been tested on the original **FT-817** or on the **FT-818** (sold as FT-818ND). Support for those models is based on published documentation, not on testing. If you own one, please test with care and report what you find.

# 817 Monitor

> Experimental software for personal use, provided without warranty. See [Disclaimer](#disclaimer).

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

# 817 EEPROM

> **⚠️ Experimental software for personal use. 817 EEPROM writes to your radio's EEPROM.** A failed or incorrect restore can leave the radio mis-calibrated or unusable. You use it entirely at your own risk; the author accepts no responsibility for any damage. Keep your own backups and a written copy of your calibration values. See [Disclaimer](#disclaimer).

![Calibration values from a backup](docs/eeprom-calibration.png)

The FT-817's EEPROM holds everything the radio knows about itself, including the **76 soft-calibration values** set at the factory. If those are lost (a factory reset, a bad write, a failing EEPROM), the radio is out of calibration, and redoing the alignment is hard without the original numbers. 817 EEPROM keeps a verified copy and can put it back.

**Back up.** Reads the whole EEPROM (6,438 bytes, addresses 0x0000–0x1925) twice and reconciles any difference by re-reading, so a cable glitch can't give you a silently bad backup. Each backup is saved automatically in the app's backups folder as a `.817ee` file: JSON with a readable hex dump and a SHA-256 checksum. The calibration values are shown by menu name and can be copied as text, so it's easy to keep a written copy as well.

**Restore.** Open a backup, press *Compare with radio*, review what will change, confirm, restore. Its safeguards:

| Safeguard | Why |
|---|---|
| Addresses 0x00–0x05 are never written | 0–3 hold a fixed marker (`A5 5A A5 5A` on both radios tested). Published sources call these bytes a checksum and warn that changing them can make the radio wipe its EEPROM, so the app never touches them. 4–5 identify the hardware version. |
| Refuses a backup whose version bytes differ from the radio's | Stops a restore from another model or variant. |
| Warns when the backup is most likely from **another radio**: more than 8 of the 76 calibration values differ from the radio's | A radio's own backup normally matches its calibration exactly. Two FT-817NDs compared in testing differed in 64 of 76 values. To go ahead anyway you must type DIFFERENT RADIO. |
| A fresh backup of the radio is saved before anything is written | Every restore can be undone. |
| Only bytes that differ are written; each is read back, with retries | Usually a few dozen writes instead of thousands; stops if writes stop verifying. |
| The radio's current CAT baud rate (0x64 bits 7–6) is kept | Restoring a different rate would cut the link halfway through. |
| Radio must be in receive with the dial LOCK off | The FT-817 can stop answering CAT while locked. |
| A full read-back at the end | You see exactly which bytes, if any, don't match. |

Restoring **calibration from another radio** onto yours would mis-calibrate it: each radio's values are its own. The app warns you when a backup looks like it came from another radio, but it can't be certain. If you own more than one radio, keep each radio's backups in its own folder and restore only that radio's backups.

**First real-radio test (recommended):** back up, change one harmless setting on the radio (for example beep volume), then restore and check that the setting comes back.

## Install

Download the installer for your system from the [Releases](../../releases) page:

| System | File |
|---|---|
| Windows 10/11 | `.msi` or `-setup.exe` |
| macOS (Apple Silicon / Intel) | `.dmg` (`aarch64` / `x64`) |
| Linux x64 | `.deb`, `.rpm` or `.AppImage` |
| Raspberry Pi 4/5 (64-bit Raspberry Pi OS) | `arm64` `.deb` |

Each app has its own installer: `817.Monitor_…` and `817.EEPROM_…`.

The builds are not code-signed yet, so you'll see a warning the first time:

- **Windows:** SmartScreen → "More info" → "Run anyway".
- **macOS:** the first time, macOS says it can't verify the developer. Open **System Settings → Privacy & Security**, scroll down and click **Open Anyway** next to the app's name. If macOS instead says the app is **damaged**, run this in Terminal once (use `817 EEPROM.app` for the other app):
  `xattr -cr "/Applications/817 Monitor.app"`
- **Linux:** add yourself to the serial group to open the port without root: `sudo usermod -aG dialout $USER` (log out and back in).

## Source code

The full source is in this repository. Notes on building, testing and releasing are in [docs/DEVELOPING.md](docs/DEVELOPING.md).

## Disclaimer

817 Monitor and 817 EEPROM are **experimental software, written for the author's personal use** and shared with the amateur radio community in the hope that they are useful. They are provided "as is", without warranty of any kind, as stated in the [MIT License](LICENSE).

- **You use them entirely at your own risk.** The author accepts **no responsibility or liability for any damage** to your radio or other equipment, including loss of calibration, settings or memory channels, or a radio that needs repair or realignment, however it is caused.
- Parts of the FT-817/818 CAT protocol used here are **undocumented by Yaesu** and come from community research. They may be wrong for your radio model, firmware or hardware variant, and they have not been verified on every model.
- **817 EEPROM writes to the radio.** Before restoring anything, make sure you have a good backup and a written copy of the 76 soft-calibration values, and only restore backups taken from that same radio.
- Neither app is affiliated with, endorsed by or supported by Yaesu.

## Credits

Protocol knowledge comes from Clint Turner KA7OEI's FT-817 CAT and memory-map pages and from the Hamlib FT-817 backend. See [docs/PROTOCOL.md](docs/PROTOCOL.md).

Not affiliated with Yaesu, or with any maker of CAT display hardware. Yaesu, FT-817 and FT-818 are trademarks of their owners.

## License

MIT, by 4Z1YR. Test reports from FT-817 and FT-818 owners are welcome. Please include your radio model and firmware when reporting meter or flag issues.
