"""Generate opendevicecontrol NXP keyboard LED/key tables from ckb-next sources.

Usage: python tools/gen_nxp_layouts.py <ckb-next>/src > crates/opendevicecontrol-core/src/devices/nxp/layouts.rs
Mirrors ckb-next src/gui/keymap.cpp getMap() for the NXP per-key models and
takes LED indices / input bits from src/daemon/keymap.c.
"""
import re
import sys

src = sys.argv[1]

# ---- daemon keymap: index = input bit, led = lighting index ----
daemon = open(f"{src}/daemon/keymap.c", encoding="utf-8").read()
body = daemon.split("const key keymap[N_KEYS_EXTENDED] = {", 1)[1].split("};", 1)[0]
entries = []
for line in body.splitlines():
    line = line.split("//")[0]
    for m in re.finditer(r'\{\s*(0|"[^"]*")\s*,\s*([^,]+),\s*([^}]+)\}', line):
        name = None if m.group(1) == "0" else m.group(1).strip('"')
        led = m.group(2).strip()
        try:
            led = int(led, 0)
        except ValueError:
            led = None
        entries.append((name, led))
INPUT_BIT = {}
LED = {}
for i, (name, led) in enumerate(entries):
    if name is None:
        continue
    INPUT_BIT.setdefault(name, i)
    LED.setdefault(name, led)

# ---- gui K95 master map ----
gui = open(f"{src}/gui/keymap.cpp", encoding="utf-8").read()
k95src = gui.split("static const Key K95Keys[] = {", 1)[1].split("};", 1)[0]
key_re = re.compile(
    r'\{(nullptr|"(?:[^"\\]|\\.)*"),\s*(nullptr|"(?:[^"\\]|\\.)*"),\s*"([^"]+)",\s*(-?\d+),\s*(-?\d+),\s*(NS|-?\d+,\s*-?\d+),\s*(true|false),\s*(true|false)\}'
)
K95 = {}
for m in key_re.finditer(k95src):
    friendly = None if m.group(2) == "nullptr" else m.group(2)[1:-1]
    K95[m.group(3)] = dict(name=m.group(3), friendly=friendly, x=int(m.group(4)), y=int(m.group(5)),
                          led=m.group(7) == "true", inp=m.group(8) == "true")
assert len(K95) > 100, len(K95)


def key(name, x, y, led=True, inp=True, friendly=None):
    return dict(name=name, friendly=friendly, x=x, y=y, led=led, inp=inp)


def k95(layout):
    m = {k: dict(v) for k, v in K95.items()}

    def rm(*names):
        for n in names:
            m.pop(n, None)

    if layout == "jis":
        rm("bslash")
        m["bspace"]["x"] = 206
        m["rshift"]["x"] += 6
        m["space"]["x"] -= 1
        m["lshift"]["x"] = 45
        rm("bslash_iso")
        m["ralt"]["x"] += 11
        rm("rwin")
    elif layout == "abnt":
        rm("yen", "henkan", "muhenkan", "katahira", "bslash")
        m["rshift"]["x"] += 6
    elif layout == "iso":
        rm("yen", "henkan", "muhenkan", "katahira", "ro", "bslash")
    else:
        rm("yen", "henkan", "muhenkan", "katahira", "ro", "bslash_iso", "hash")
        m["enter"]["x"] = 199
        m["enter"]["y"] = 51
        m["lshift"]["x"] = 45
    return m


def k95p(layout):
    m = k95(layout)
    for i in range(7, 19):
        m.pop(f"g{i}", None)
    for i in range(1, 7):
        m[f"g{i}"]["x"] = 22
    for i, y in zip(range(2, 7), (26, 38, 50, 62, 74)):
        m[f"g{i}"]["y"] = y
    for n in ("m1", "m2", "m3", "mr"):
        m.pop(n)
    m["profswitch"] = key("profswitch", 64, 1, friendly="Profile Switch")
    m["light"]["x"] = 75
    m["lock"]["x"] = 86
    for n in ("mute", "voldn", "volup"):
        m[n]["x"] -= 11
    for v in m.values():
        v["x"] -= 20
        v["y"] += 6
    bar = [("topbar1", 4), ("topbar2", 19), ("topbar3", 34), ("topbar4", 49), ("topbar5", 64), ("topbar6", 79),
           ("topbar7", 94), ("topbar8", 109), ("topbar9", 124), ("topbar10", 139), ("topbar11", 154),
           ("topbar12", 169), ("topbar13", 184), ("topbar14", 199), ("topbar15", 214), ("topbar16", 229),
           ("topbar17", 244), ("topbar18", 259), ("topbar19", 274)]
    for i, (n, x) in enumerate(bar):
        m[n] = key(n, x, -3, inp=False, friendly=f"Light bar {i + 1}")
    return m


def k70(layout):
    m = k95(layout)
    out = {}
    for n, v in m.items():
        v["x"] -= 38
        if v["x"] >= 0:
            out[n] = v
    for n in ("mr", "m1", "m2", "m3"):
        out.pop(n, None)
    return out


def k70mk2(layout):
    m = k70(layout)
    m["profswitch"] = key("profswitch", 40, 0, friendly="Profile Switch")
    m["light"]["x"] = 49
    m["lock"]["x"] = 58
    m["m3"] = key("m3", 118, 0, inp=False, friendly="Logo 1")
    m["g11"] = key("g11", 138, 0, inp=False, friendly="Logo 2")
    for n in ("mute", "volup", "voldn"):
        m[n]["x"] -= 10
    return m


def strafe_mk2(layout):
    m = k70mk2(layout)
    for v in m.values():
        v["x"] += 12
    for n in ("profswitch", "light", "lock"):
        m[n]["x"] -= 20
    return m


def k68(layout):
    m = k70(layout)
    # The K68 has only mute + volume keys; their LEDs reuse the stop/prev
    # indices, so drop the K70 media keys ckb's GUI map leaves in.
    for n in ("stop", "prev", "play", "next"):
        m.pop(n, None)
    m["voldn"] = key("voldn", 285 - 38, 0, led=True, friendly="Volume Down")
    m["volup"] = key("volup", 297 - 38, 0, led=True, friendly="Volume Up")
    return m


def k65(layout):
    m = {n: v for n, v in k70(layout).items() if v["x"] < 209}
    for n, x, led, fr in (("light", 164, True, "Brightness"), ("mute", 176, True, "Mute"),
                          ("voldn", 192, True, "Volume Down"), ("volup", 205, True, "Volume Up"),
                          ("lock", 222, True, "Windows Lock")):
        m[n] = key(n, x - 38, 0, led=led, friendly=fr)
    m.pop("rwin", None)
    m["fn"] = key("fn", 152 - 12, 75, friendly="Function")
    return m


def strafe(layout):
    m = k70(layout)
    m["light"]["x"] = 285 - 38
    m["lock"]["x"] = 297 - 38
    for v in m.values():
        v["x"] += 12
    m["logo"] = key("logo", 12, 0, inp=False, friendly="Logo")
    m["fn"] = key("fn", 152, 75, friendly="Function")
    for n in ("rwin", "mute", "volup", "voldn", "stop", "prev", "play", "next"):
        m.pop(n, None)
    return m


MODELS = {
    "K95": k95, "K95P": k95p, "K70": k70, "K70MK2": k70mk2, "STRAFE_MK2": strafe_mk2,
    "K68": k68, "K65": k65, "STRAFE": strafe,
}
LAYOUTS = [("ansi", 1), ("iso", 2), ("abnt", 4), ("jis", 8)]

# The volume barrel has no LED (ckb gui: hasLed false), K65/K68 volume keys
# also share LEDs with media keys; follow ckb's hasLed flags.
for v in K95.values():
    pass

NICE = {
    "esc": "Esc", "grave": "`", "minus": "-", "equal": "=", "bspace": "Backspace", "tab": "Tab",
    "lbrace": "[", "rbrace": "]", "bslash": "\\\\", "caps": "Caps Lock", "colon": ";", "quote": "'",
    "hash": "# (ISO)", "enter": "Enter", "lshift": "Left Shift", "bslash_iso": "\\\\ (ISO)", "comma": ",",
    "dot": ".", "slash": "/", "rshift": "Right Shift", "lctrl": "Left Ctrl", "lwin": "Left Win",
    "lalt": "Left Alt", "space": "Space", "ralt": "Right Alt", "rwin": "Right Win", "rmenu": "Menu",
    "rctrl": "Right Ctrl", "prtscn": "Print Screen", "scroll": "Scroll Lock", "pause": "Pause",
    "ins": "Insert", "home": "Home", "pgup": "Page Up", "del": "Delete", "end": "End", "pgdn": "Page Down",
    "up": "Up", "left": "Left", "down": "Down", "right": "Right", "numlock": "Num Lock",
    "numslash": "Num /", "numstar": "Num *", "numminus": "Num -", "numplus": "Num +",
    "numenter": "Num Enter", "numdot": "Num .", "light": "Brightness", "lock": "Win Lock",
    "mute": "Mute", "volup": "Volume Up", "voldn": "Volume Down", "stop": "Stop", "prev": "Previous",
    "play": "Play/Pause", "next": "Next", "mr": "MR", "m1": "M1", "m2": "M2", "m3": "M3",
    "profswitch": "Profile", "fn": "Fn", "logo": "Logo", "yen": "¥", "ro": "Ro", "henkan": "Henkan",
    "muhenkan": "Muhenkan", "katahira": "Katakana/Hiragana",
}


def nice(k):
    if k.get("friendly") and k["name"] in ("m3", "g11") or k["name"].startswith("topbar"):
        return k["friendly"]
    n = k["name"]
    if n in NICE:
        return NICE[n]
    if n.startswith("num") and n[3:].isdigit():
        return "Num " + n[3:]
    return n.upper()


SPECIAL = ["g%d" % i for i in range(1, 19)] + ["mr", "m1", "m2", "m3", "profswitch", "light", "lock",
                                                "mute", "volup", "voldn", "stop", "prev", "play", "next"]

MEDIA = {"mute": ("mute", "Mute"), "volup": ("volume_up", "Volume up"), "voldn": ("volume_down", "Volume down"),
         "stop": ("stop", "Stop"), "prev": ("previous", "Previous track"), "play": ("play_pause", "Play/Pause"),
         "next": ("next", "Next track")}


def KEYINFO(n):
    """(id, name, assignable, hardware-mode action, software routing)"""
    if n.startswith("g"):
        return n, n.upper(), True, None, "ROUTE_HOST"
    if n == "mr":
        return "mr", "MR (macro record)", True, "ACT_MR", "ROUTE_HOST"
    if n in ("m1", "m2", "m3"):
        return n, n.upper(), True, f"ACT_{n.upper()}", "ROUTE_HOST"
    if n == "profswitch":
        return "profile", "Profile", True, None, "ROUTE_HOST"
    if n == "light":
        return "brightness", "Brightness", False, "ACT_LIGHT", "ROUTE_HOST"
    if n == "lock":
        return "win_lock", "Win lock", False, "ACT_LOCK", "ROUTE_HOST"
    kid, name = MEDIA[n]
    # Media keys keep working in the OS (HID) and are also reported to us.
    return kid, name, False, None, "ROUTE_BOTH"


sys.stdout.reconfigure(encoding="utf-8", newline="\n")
out = sys.stdout
out.write("// @generated by tools/gen_nxp_layouts.py from ckb-next (GPL-2.0)\n")
out.write("// src/gui/keymap.cpp and src/daemon/keymap.c. Do not edit by hand; regenerate with\n")
out.write("// `python tools/gen_nxp_layouts.py <ckb-next>/src > crates/opendevicecontrol-core/src/devices/nxp/layouts.rs`.\n")
out.write("//\n// Per model: LEDs (lighting index, ckb GUI position, label, layout mask:\n")
out.write("// 1 ANSI, 2 ISO, 4 ABNT, 8 JIS) sorted left to right, and the special keys.\n\n")
out.write("use super::*;\n\n")
for model, fn in MODELS.items():
    merged = {}
    keys = {}
    for lname, bit in LAYOUTS:
        m = fn(lname)
        for k in m.values():
            n = k["name"]
            led = LED.get(n)
            if k["led"] and led is not None and led >= 0:
                ident = (n, led, k["x"], k["y"])
                merged[ident] = merged.get(ident, 0) | bit
            if k["inp"] and n in SPECIAL:
                keys[n] = INPUT_BIT[n]
    rows = sorted(merged.items(), key=lambda e: (e[0][2], e[0][3], e[0][1]))
    # sanity: unique LEDs per layout
    for lname, bit in LAYOUTS:
        leds = [r[0][1] for r in rows if r[1] & bit]
        assert len(leds) == len(set(leds)), (model, lname, sorted(leds))
    by_name = {}
    for lname, bit in LAYOUTS:
        for k in fn(lname).values():
            by_name.setdefault(k["name"], k)
    out.write(f"pub(super) static {model}_LEDS: &[KbLed] = &[\n")
    for (n, led, x, y), mask in rows:
        label = nice(by_name[n]).replace('"', '\\"')
        out.write(f'    KbLed {{ led: 0x{led:02x}, x: {x}, y: {y}, name: "{label}", layouts: {mask:#06b} }},\n')
    out.write("];\n")
    out.write(f"pub(super) static {model}_KEYS: &[SpecialKey] = &[\n")
    for n in sorted(keys, key=lambda n: SPECIAL.index(n)):
        kid, kname, assignable, action, route = KEYINFO(n)
        act = f"Some({action})" if action else "None"
        out.write(f'    SpecialKey {{ bit: {keys[n]}, id: "{kid}", name: "{kname}", assignable: {str(assignable).lower()}, '
                  f'hw_action: {act}, route: {route} }},\n')
    out.write("];\n\n")

