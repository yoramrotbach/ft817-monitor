# FT-817/818 CAT: what the apps read and write

All frames are 5 bytes: four parameter bytes, then the opcode. The link is 8N2 at 4800, 9600 or 38400 baud.

Sources:
- **[Y]** Yaesu FT-817 operating manual, CAT section
- **[K]** KA7OEI, "CAT interface programming with the FT-817" and "FT-817 memory map" (ka7oei.com)
- **[H]** Hamlib `rigs/yaesu/ft817.c` (field-tested decoding used by WSJT-X and others)

## Commands sent (the complete list)

| Opcode | Name | Reply | Polled | Source |
|---|---|---|---|---|
| `03` | Read frequency & mode | 5 bytes | every cycle | Y, K, H |
| `F7` | Read TX status | 1 byte | every cycle | Y, K, H |
| `E7` | Read RX status | 1 byte | every cycle in RX | Y, K, H |
| `BD` | Read TX metering (undocumented) | 2 bytes in TX, 1 byte `00` in RX | every cycle in TX, if enabled | K, H |
| `BB` | Read EEPROM (undocumented) | 2 bytes (addr, addr+1) | every 700 ms | K, H |

`frame()` in `cat.rs` rejects every other opcode. A unit test checks that `BC` (EEPROM write) and `BE` (factory reset) are refused.

## Decoding

**`03` frequency/mode.** Bytes 0–3 are packed BCD, MSB first, in 10 Hz units (`01 40 74 00` = 14.074.00 MHz). Byte 4 is the mode: `00` LSB, `01` USB, `02` CW, `03` CWR, `04` AM, `06` WFM, `08` FM, `0A` DIG, `0C` PKT. Bit 7 set means narrow (for example `82` = CW-N, `88` = FM-N). **A real FT-817ND reports PKT as `FC`**, which the published sources don't mention; the app treats both `0C` and `FC` as PKT, and shows any other unknown value as `?xx` without a narrow flag.

**`F7` TX status.** The radio answers `FF` in receive; Hamlib uses this to detect PTT, and so does this app. In transmit: bits 0–3 = power output (0–15), bit 6 = high SWR. Bit 5 (split) is **not used**: [K] notes reports that bits 5 and 7 may be swapped. Split comes from EEPROM `7A` instead.

**`E7` RX status** (valid in receive only). Bits 0–3 = S-meter (0–9 = S0–S9, 10 = S9+10 dB … 15 = S9+60 dB); bit 5 = FM discriminator off-center; bit 6 = CTCSS/DCS enabled but not matched; bit 7 = squelched.

**`BD` TX meters.** Two bytes, four nibbles. Published sources disagree on the layout. The app uses the one that matched the radio's own meters on a real FT-817ND:

| byte 0 high | byte 0 low | byte 1 high | byte 1 low |
|---|---|---|---|
| PWR | ALC | SWR | MOD |

Field test (Nov 2026, direct serial, radio's METER switched between SWR, ALC and MOD for comparison): in AM into the antenna the radio showed SWR 0, ALC full and MOD full, and the reply nibbles were SWR 0, ALC 6, MOD 5. With the antenna disconnected, SWR rose to 5–7, as on the radio. Keyed in AM without speaking, MOD dropped to 0, as on the radio. KA7OEI's layout (PWR|SWR, MOD|ALC) did not match. Hamlib's decoding of SWR and ALC matches; its MOD reads the wrong nibble, but Hamlib doesn't report MOD. Not tested: the FT-818.

**`BB` EEPROM** (address in bytes 0–1). The app uses only the first returned byte, as Hamlib does.

| Addr | Bits | Meaning | Source |
|---|---|---|---|
| `55` | 0 VFO A/B · 1 MTQMB · 2 QMB · 4 HOME · 5 MTUNE · 7 VFO(1)/MEM(0) | VFO/memory state | K, H (bit 0) |
| `57` | 1-0 AGC (Auto/Fast/Slow/Off) · 2 DSP · 4 PBT · 5 NB · 6 LOCK · 7 FST | panel | K |
| `58` | 1-0 meter mode (PWR/ALC/SWR/MOD) · 4 KYR · 5 BK · 6 VLT · 7 VOX | panel | K |
| `5B` | 4 LCD color (0 blue, 1 amber) | display | K |
| `65` | 7-5 DIG sub-mode (RTTY, PSK-L, PSK-U, USER-L, USER-U) | read only in DIG | K, H |
| `79` | 1-0 TX power (HI, L3, L2, L1) | | K |
| `7A` | 0 HF · 1 6 m · 2 FM BC · 3 Air · 4 2 m · 5 UHF · 6 general coverage antenna (0 front, 1 rear) · 7 split | | K, H |

**Antenna:** the band group is inferred from the frequency (below 50 MHz = HF, 50–56 = 6 m, 76–108 = FM BC, 108–137 = Air, 144–148 = 2 m, 420–470 = UHF, anything else = general coverage). Edge cases outside the amateur bands may not match the radio's band-stack logic.

## EEPROM backup and restore (817 EEPROM only)

817 EEPROM uses `BB` to read and `BC` to write. A `BC` frame is `addrHi addrLo data0 data1 BC`: it always writes **two** bytes (the address and the next one up) **[K]**. The radio normally answers with one acknowledgment byte, but Hamlib tolerates a missing one **[H]**, and so does this app: what counts is reading the pair back.

| Fact | Source | How the app uses it |
|---|---|---|
| Valid addresses are 0x0000–0x1925; reading past the end returns a single `00` | K | The backup reads exactly 0x0000–0x1925 and never asks past the end: on a real FT-817 the out-of-range reply was not a single `00` (field report, Nov 2026), so probing for the end was unreliable |
| 0x00–0x03 are described as an integrity checksum; changing them can wipe the EEPROM, calibration included. On the two FT-817NDs tested they are a fixed `A5 5A A5 5A` (see field notes below) | K | `write_frame()` refuses any address below 0x06 (unit tested); the simulator wipes itself if those bytes change |
| 0x04–0x05 identify the hardware version | K | A restore is refused if they differ between backup and radio |
| 0x07–0x52 = soft-cal menu items 1–76 (address = item + 6); VCC calibration at 0x53; items 75–76 are signed | K | Shown by name in the app; counted as "calibration" in the restore summary |
| 0x64 bits 7–6 = CAT rate (00 = 4800, 01 = 9600, 10 = 38400) | K | Kept at the radio's current value during a restore |
| No checksum on the serial link; the radio can miss commands while busy | K | Two-pass reads with re-reads; every write read back, up to 3 tries; full read-back at the end |
| The FT-817 may stop answering CAT while the dial is LOCKed | K | Restore and backup refuse to start while LOCK is on |

Region labels in the restore summary (calibration, settings, VFOs/home, memories) follow the FT-817 map and are approximate on the FT-818.

## FT-818 notes

Hamlib reads `55`, `59`, `65` and `7A` at the same addresses on both radios. The `57`, `58`, `5B` and `79` addresses come from the FT-817 memory map and have **not been verified on an FT-818**. The FT-818 inserts 60 m into the band table, which shifts the band index at `59`; this app avoids that by deriving the band from the frequency.

## rigctld mode

Commands used: `f`, `m`, `t`, `v`, `s`, `l RAWSTR` (receive), `\get_dcd`, and `l RFPOWER_METER`, `l SWR`, `l ALC` (transmit). Errors (`RPRT -n`) for unsupported items are shown as "not available". The TX meter values are scaled to the 0–15 bar and are approximate.

## Field notes from real radios (Nov 2026)

Two FT-817ND radios, both unmodified, bought from different owners:

- Bytes 0x00–0x03 read `A5 5A A5 5A` on both radios, so they look like a fixed marker rather than a per-radio checksum. The app still never writes them.
- Version bytes (0x04–0x05) differ between the two: `CE BF` and `D8 BF`.
- 64 of the 76 soft-calibration values differ between the two radios. This is the basis for the app's "most likely another radio" warning (threshold: more than 8 differing values).
- The first radio tested did not answer an EEPROM read past 0x1925 with the single `00` byte described in published sources, so the app now reads exactly 0x0000–0x1925 and never asks past the end.

Not tested: the original FT-817 and the FT-818.
