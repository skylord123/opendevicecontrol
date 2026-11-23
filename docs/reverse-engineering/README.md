# Reverse engineering notes

This folder collects the protocol and firmware notes behind OpenDeviceControl's
device support, so the work does not live only in code comments and commit
messages. It is meant for anyone (people or language models) who wants to
understand, check, or extend a driver. There is one page per product.

How the work was done, in general:

- Static analysis of shipped firmware and of vendor software, plus the public
  protocol work of OpenRGB, ckb-next, liquidctl and OpenLinkHub.
- On-device confirmation over USB or I2C where the hardware was on hand. Each
  page says what was checked on hardware and what is inference.
- No firmware was ever flashed, and no bootloader or update command was sent to
  a real device. Commands that would write flash or enter a bootloader are
  documented so we can be sure to avoid them, not to use them.

The driver code is under `crates/opendevicecontrol-core/src/vendors/`.

## Pages

- [Corsair K55 RGB keyboard (NXP firmware)](corsair-k55.md): firmware study, HID command set and key routing for the K55.
- [Corsair NXP family (shared protocol)](corsair-nxp.md): protocol shared by Corsair devices on NXP firmware.
- [ASRock Polychrome RGB (USB)](asrock-polychrome.md): USB protocol for ASRock Polychrome RGB controllers.
- [ASUS Aura (ENE) lighting](asus-aura.md): ENE-based ASUS Aura lighting over I2C.
- [ASUS Aura motherboard lighting (USB)](asus-aura-usb.md): config table, effects and direct colours of ASUS boards' USB controllers, based on OpenRGB; not tested on hardware.
- [MSI Mystic Light (USB)](msi-mystic-light.md): the 162- and 185-byte feature-report protocols and per-board zone tables, based on OpenRGB; not tested on hardware.
- [Gigabyte RGB Fusion 2 (USB)](gigabyte-rgb-fusion2.md): info report, effects and direct colours of Gigabyte's ITE controller, based on liquidctl and OpenRGB; not tested on hardware.
- [Aula F99 keyboard](aula-f99.md): notes on the Aula F99 keyboard protocol.
- [Bose QuietComfort Headphones (BMAP)](bose-qc-headphones.md): the Bose Music app's Bluetooth protocol, recovered from the Android app and tested on the headphones, with per-block notes in [bose-bmap/](bose-bmap/).
- [Logitech HID++ (mice, keyboards, receivers)](logitech-hidpp.md): receiver registers, HID++ 2.0 feature layouts, button diversion and gestures, based on Solaar and tested on an MX Master 3S and a Performance Mouse MX.
- [Speedlink ACCUSOR keyboard](speedlink-accusor.md): feature commands, key table, onboard macros and events, recovered from the Windows driver and tested on hardware.
