# BeagleBone-V2H

### My Modifications
- Added MQTT support for meter readings (alongside existing Modbus) - Supports Hildebrand Glow, mbmd and other MQTT-based meters.
  
---

Functional vehicle to home bidirectional charging app based on Indra CHAdeMO home charger

Tested on Debian 10 - BeagleBone Green [SD image](https://www.beagleboard.org/distros/am3358-debian-10-3-2020-04-06-1gb-sd-console)

Custom kernel hardware modules and configuration can be found in ./supporting

Requires:

* External grid energy feed for V2H

WIP:

* Internal STP3x energy monitor for V2H

Todo:

* ADC SPI driver for differential voltage across contactors, welding checks etc
* Review CHAdeMO shutdown procedure (OBD2 codes thrown)

Crosscompile using [ZigBuild](https://github.com/rust-cross/cargo-zigbuild)

```cargo zigbuild --target arm-unknown-linux-musleabihf --release```
