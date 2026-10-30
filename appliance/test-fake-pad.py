"""Create a virtual game pad, send a scripted sequence of events, remove it.

usage: test-fake-pad.py ACTION...   where an ACTION is
  hat:x:-1 | hat:y:1 ...      the D-pad hat (ABS_HAT0X / ABS_HAT0Y), -1, 0 or 1
  stick:x:0 | stick:y:65535   the left stick / D-pad-as-stick (range 0..65535, centre 32768)
  btn:south:1 | btn:east:0    buttons (value 1 pressed, 0 released)
The device exists only for the duration of the script, and is created first with a pause, so the
UI's device rescan can find it (a pad that connects after the UI started).
"""
import sys
import time

from evdev import AbsInfo, UInput
from evdev import ecodes as e

caps = {
    e.EV_KEY: [e.BTN_SOUTH, e.BTN_EAST, e.BTN_START],
    e.EV_ABS: [
        (e.ABS_X, AbsInfo(32768, 0, 65535, 0, 0, 0)),
        (e.ABS_Y, AbsInfo(32768, 0, 65535, 0, 0, 0)),
        (e.ABS_HAT0X, AbsInfo(0, -1, 1, 0, 0, 0)),
        (e.ABS_HAT0Y, AbsInfo(0, -1, 1, 0, 0, 0)),
    ],
}
pad = UInput(caps, name="Test Game Pad", vendor=0x2DC8, product=0x5112)
time.sleep(5)
AXES = {"hat:x": e.ABS_HAT0X, "hat:y": e.ABS_HAT0Y, "stick:x": e.ABS_X, "stick:y": e.ABS_Y}
BTNS = {"south": e.BTN_SOUTH, "east": e.BTN_EAST, "start": e.BTN_START}
for action in sys.argv[1:]:
    kind, name, value = action.split(":", 2) if action.count(":") == 2 else (None, None, None)
    if action.startswith("btn:"):
        _, name, value = action.split(":")
        pad.write(e.EV_KEY, BTNS[name], int(value))
    else:
        axis, value = action.rsplit(":", 1)
        pad.write(e.EV_ABS, AXES[axis], int(value))
    pad.syn()
    time.sleep(0.4)
time.sleep(1)
pad.close()
