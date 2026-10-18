#!/usr/bin/env bash
# Read-only hardware probe for the media-appliance spike. Run it ON the NUC, from
# your workstation, with one command (nothing is copied to the NUC):
#
#   git show origin/claude/peaceful-cannon-y734g3:tools/probe-hardware.sh \
#     | ssh nuc 'nix shell nixpkgs#libva-utils nixpkgs#libdrm nixpkgs#edid-decode nixpkgs#alsa-utils nixpkgs#usbutils nixpkgs#pciutils nixpkgs#v4l-utils -c bash -c "$(cat)"' \
#     | tee probe.txt
#
# It only reads: no sudo, no writes, no network. It skips serial numbers and MAC
# addresses, but read probe.txt before sharing it. Missing tools are reported,
# not fatal.
have() { command -v "$1" >/dev/null 2>&1; }
section() { printf '\n######## %s\n' "$1"; }
run() { # run "label" cmd...
  printf '\n$ %s\n' "$*"; "$@" </dev/null 2>&1 | head -${LINES_MAX:-80}
}
note_missing() { printf '\n(%s not installed: nix shell nixpkgs#%s)\n' "$1" "$2"; }

section "system"
run uname -srmo
[ -r /etc/os-release ] && grep -E '^(PRETTY_NAME|VERSION_ID|BUILD_ID)=' /etc/os-release
have nixos-version && run nixos-version
run lscpu | grep -E 'Model name|^CPU\(s\)|Thread|Max MHz' 2>/dev/null
grep -E 'MemTotal' /proc/meminfo
cat /proc/cmdline | tr ' ' '\n' | grep -vE 'init=|root=|^$' | head -20

section "graphics hardware"
if have lspci; then lspci -nnk | grep -A3 -Ei 'vga|display|3d|audio' | head -40; else note_missing lspci pciutils; fi
ls -l /dev/dri 2>&1
for d in /sys/class/drm/card*/device/driver; do [ -e "$d" ] && printf '%s -> %s\n' "$d" "$(readlink "$d")"; done

section "display: connectors, modes, HDR (what the TV reports)"
for c in /sys/class/drm/card*-*; do
  [ -d "$c" ] || continue
  printf '\n== %s status=%s enabled=%s\n' "$(basename "$c")" "$(cat "$c/status" 2>/dev/null)" "$(cat "$c/enabled" 2>/dev/null)"
  [ "$(cat "$c/status" 2>/dev/null)" = connected ] || continue
  printf 'modes (first 25):\n'; head -25 "$c/modes" 2>/dev/null
  if have edid-decode && [ -r "$c/edid" ]; then
    edid-decode "$c/edid" 2>&1 | grep -iE 'Manufacturer|Model|Display Product Name|Maximum image|HDR|Dolby|HLG|SMPTE ST 2084|Static Metadata|Colorimetry|BT.2020|Deep Color|Supported|VRR|Frequency|Vertical|refresh|Vendor-Specific|HDMI Forum|FRL|ALLM|Audio|Short Audio' | grep -viE 'serial' | head -70
  elif [ -r "$c/edid" ]; then printf 'EDID present (%s bytes); install edid-decode to read it\n' "$(wc -c < "$c/edid")"; fi
done
if have modetest; then run modetest -c; else note_missing modetest libdrm; fi

section "video decode (VA-API): which codecs the iGPU decodes in hardware"
if have vainfo; then
  for r in /dev/dri/renderD*; do [ -e "$r" ] && run vainfo --display drm --device "$r"; done
else note_missing vainfo libva-utils; fi

section "audio: sinks and what the HDMI sink/AVR can accept (ELD)"
have aplay && run aplay -l || note_missing aplay alsa-utils
for e in /proc/asound/card*/eld#*; do [ -r "$e" ] && { printf '\n== %s\n' "$e"; grep -E 'monitor_present|eld_valid|monitor_name|connection_type|sad[0-9]*_(coding_type|channels|rates|bits)' "$e" 2>/dev/null; }; done
have wpctl && run wpctl status
have pactl && run pactl list short sinks

section "remote control / CEC / input"
have lsusb && run lsusb || note_missing lsusb usbutils
ls -l /dev/cec* 2>&1 | head
have cec-ctl && run cec-ctl --list-devices
ls /dev/lirc* /sys/class/rc 2>&1 | head
printf '\ninput devices (names only):\n'; grep -E '^N: Name=' /proc/bus/input/devices

section "current media software"
for b in kodi mpv ffmpeg ffprobe retroarch yt-dlp deno; do printf '%-10s %s\n' "$b" "$(command -v $b || echo -)"; done
have mpv && run mpv --version
have systemctl && systemctl list-units --type=service --no-legend 2>/dev/null | grep -iE 'kodi|youtube|retroarch|cage|greetd|display-manager' | head
have retroarch && run retroarch --version

section "storage layout (no serials)"
have lsblk && run lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS
echo; echo "done: $(date -u +%FT%TZ)"
