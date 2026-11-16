# Lighting

Every device with LEDs has a Lighting tab on its own page. The **Lighting**
page at the top of the window brings them together.

## The Lighting page

- **Sync:** tick the devices that should run one effect together. The
  others keep the effect set on their own page, so the keyboard can do its
  own thing while the board and the GPU match.
- **Layout:** a picture of your PC. Every fan, strip and keyboard is drawn
  with its live colours. Drag things to where they sit in your case, click
  one to turn or resize it, or drag a box (or Shift-click) to move several
  at once. Synced devices run their effect across this layout, so a wave
  sweeps from the front fans to the back of the case.

## Fans and strips

On a device with lighting headers (like an ASRock board), **What's plugged
in** describes each header: add a fan or a strip, say how many LEDs it has
and which LED it starts at. Fans on a splitter that all light alike are one
fan with several copies; each copy gets its own place on the layout.

Zones you don't use can be hidden, and any zone can be renamed (say "Rear
case fan"); the board's own name stays visible next to yours.

## Separate effects

Under the device's effect, each channel has a box. **Own effect** gives the
channel an effect of its own, and **Separate fans** splits it further, a
different effect per fan or strip. This works whether or not the device is
synced. Giving one fan static white is a quick way to find which one it is.

## Effects

Plain effects: Static, Breathing, Color cycle, Rainbow wave, Color wave,
Pacifica, Twinkle and Meteor.

Sound-reactive effects follow whatever the computer is playing: Spectrum,
Beat pulse, Ripple, Sound stream, Rain, Level meter, Radar, Level colour,
Audio fire, Gravity meter, Frequency star, Spectrum bubbles, Pitch stream,
DJ light, Noise move, Audio plasma and Spectrogram. Their colours can be a
rainbow, two colours of your own, or **Notes**: the colours of the musical
notes being played.

The effects **by musical note** (Notes, Note cells, Note fields, Main note)
find the notes in the music, fold every octave together, and give each note
its own colour: A is yellow, C# red, F blue, and the rest in between. A
chord shows as its own mix of colours.

## Credits

- The note effects follow the method of
  [ColorChord](https://github.com/cnlohr/colorchord) by Charles Lohr (used
  under its MIT option). OpenDeviceControl has its own implementation.
- Several effects are inspired by [WLED](https://github.com/wled/WLED)'s
  sound-reactive effects, the
  [OpenRGB Effects Plugin](https://gitlab.com/OpenRGBDevelopers/OpenRGBEffectsPlugin)
  and [FastLED](https://github.com/FastLED/FastLED)'s examples (Fire 2012,
  Pacifica, Twinklefox). They are written from scratch here, with our own
  palettes and timings.
