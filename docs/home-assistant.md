# Home Assistant

OpenDeviceControl uses **MQTT discovery**: devices and entities appear in Home
Assistant by themselves, with unique IDs, units, device classes and long-term
statistics, and controls work both ways.

## Setup

1. In Home Assistant, install the **Mosquitto broker** add-on and enable the
   **MQTT** integration (or use any MQTT broker you already have).
2. In OpenDeviceControl → **Settings → Home Assistant**, tick *Enable*, enter
   the broker host, port and username/password, then click **Save**.
3. Your devices appear under *Settings → Devices & services → MQTT*.

## What is sent

**Only what you choose.** The house column of the sensor tree shows which
sensors are sent and toggles them (per sensor or per group); *Settings → Home
Assistant → Choose sensors…* opens the same tree with check boxes. Out of the
box OpenDeviceControl sends presence, CPU load and temperature, memory, GPU,
disks, network, uptime and every device's own sensors; the detail sensors (per
core, per rail, per drive…) are one click away. Sensors you switch off are
removed from Home Assistant.

- **Controls from HA** can be turned off with *Allow Home Assistant to change
  fan and PSU settings*.
- **Availability:** entities go *unavailable* when OpenDeviceControl exits or
  the PC is off (an MQTT last-will, so it also works after a crash).
- **Energy dashboard:** add the PSU's *Energy* sensor as an individual device or
  as grid consumption.

## Entities

HXi/RMi PSU:

| Type | Entities |
|---|---|
| `sensor` | Input voltage, Input power*, Output power, Efficiency*, Energy* (kWh, `total_increasing`), +12V/+5V/+3.3V voltage, current and power, VRM temperature, Case temperature, Fan speed, Fan duty |
| `sensor` (diagnostic) | Uptime, Total uptime |
| `select` | PSU fan mode (Hardware / Fixed / Curve), +12V OCP mode |
| `number` | PSU fan fixed duty (30–100 %) |

\* estimated, see [devices.md](devices.md#corsair-hxi--rmi-power-supplies).

Keyboard (K55 RGB):

| Type | Entities |
|---|---|
| `light` | Lighting: on/off, brightness, colour, effect |
| `binary_sensor` | "G1 pressed" …: `on` while held, sent the instant it changes |
| `event` | "G1 key" …: fires `press` and `release` |
| `switch` | Win lock, and whether it also blocks Alt+Tab, Alt+F4 and Shift+Tab |

Legacy headset (VOID family, HS70):

| Type | Entities |
|---|---|
| `sensor` / `binary_sensor` | Battery, Charging (wireless only) |
| `light` | Lighting (models with logo LEDs) |
| `number` | Sidetone (0–100 %) |
| `switch` | Mic mute LED |
| `select` | Play notification sound |

Example: toggle a light when G1 is held for 3 seconds.

```yaml
triggers:
  - trigger: state
    entity_id: binary_sensor.corsair_k55_rgb_g1_pressed
    to: "on"
    for: { seconds: 3 }
actions:
  - action: light.toggle
    target: { entity_id: light.office }
```

## Lighting

Every device shows up as connected through the computer (named after it,
so several PCs with the same parts stay apart), and lighting comes as
ordinary Home Assistant lights:

| Entity | On | What it controls |
|---|---|---|
| `light` "PC lighting" | the computer | The shared effect of the devices synced on the Lighting page |
| `light` "Lighting" | each device with LEDs | The device's own effect (the shared one while it is synced) |
| `light` "*Channel* lighting", "*Channel* *Fan 1* lighting" | the device | A channel, fan or strip given an effect of its own in the app |
| `switch` "Follow PC lighting" | each device with LEDs | Whether the device runs the shared effect |

Each light has on/off, brightness, a colour and an effect list with every
effect by its name in the app (`Rainbow wave`, `Spectrum`, `Audio fire`,
`Notes`...). On devices with their own built-in effects (an ASRock board,
the F99, an ASUS card), the "OpenDeviceControl lighting" switch hands the
LEDs back to the device.

Lights off while nobody is home, back on when someone arrives:

```yaml
triggers:
  - trigger: state
    entity_id: zone.home
    to: "0"
    id: away
  - trigger: numeric_state
    entity_id: zone.home
    above: 0
    id: home
actions:
  - action: light.turn_{{ 'off' if trigger.id == 'away' else 'on' }}
    target: { entity_id: light.my_pc_pc_lighting }
```

Sound-reactive lighting while music plays, a calm effect after:

```yaml
triggers:
  - trigger: state
    entity_id: media_player.living_room
    to: playing
    id: music
  - trigger: state
    entity_id: media_player.living_room
    from: playing
    id: stopped
actions:
  - action: light.turn_on
    target: { entity_id: light.my_pc_pc_lighting }
    data:
      effect: "{{ 'Notes' if trigger.id == 'music' else 'Pacifica' }}"
```

Let the keyboard do its own thing while a game runs:

```yaml
actions:
  - action: switch.turn_off
    target: { entity_id: switch.my_pc_aula_f99_follow_pc_lighting }
  - action: light.turn_on
    target: { entity_id: light.my_pc_aula_f99_lighting }
    data: { effect: Static, rgb_color: [255, 40, 0] }
```

Entity ids depend on the names Home Assistant gave the entities; check
yours under Settings → Devices & services → MQTT.

## Dual boot

Run OpenDeviceControl on both operating systems with the **same Node ID**
(Settings → Home Assistant). Both publish to one HA device, so history and the
energy counter carry on whichever OS is booted. On connect, OpenDeviceControl
continues from the larger of its local energy counter and the value retained
on the broker, so the `total_increasing` sensor never goes backwards.

## Why MQTT and not the WebSocket/REST API?

Home Assistant's WebSocket and REST APIs cannot create devices or registry
entities: the registry endpoints only update or remove existing entries,
`POST /api/states` makes state-only entities that can't be renamed or
restored, and selects/numbers made that way can't send commands back. MQTT
discovery is the supported way to get first-class entities with two-way
control. (The ESPHome native API would also work without a broker and may
become an option later.)
