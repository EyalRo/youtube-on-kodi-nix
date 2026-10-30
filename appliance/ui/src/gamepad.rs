//! Game controllers as a remote: turn their D-pad, left stick and buttons into UI navigation.
//!
//! Pads differ in how they report the D-pad (a hat on ABS_HAT0X/Y, or the left stick on ABS_X/Y, as
//! the 8BitDo M30 does by default over Bluetooth), so both are read. The mapping here is pure and
//! unit-tested; `main.rs` reads the evdev devices and injects the resulting key presses.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
}

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_HAT0X: u16 = 16;
const ABS_HAT0Y: u16 = 17;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_START: u16 = 0x13b;

/// True when the key capability bitmap of an input device (the `capabilities/key` sysfs file: hex
/// words, most significant first) has BTN_SOUTH, which every game pad reports and no keyboard,
/// mouse or remote does.
pub fn is_gamepad(key_capabilities: &str) -> bool {
    has_bit(key_capabilities, BTN_SOUTH as usize)
}

fn has_bit(words_msb_first: &str, bit: usize) -> bool {
    let words: Vec<u64> = words_msb_first.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
    let n = words.len();
    let (idx, off) = (bit / 64, bit % 64);
    idx < n && words[n - 1 - idx] >> off & 1 == 1
}

/// One stick or D-pad axis: its range (from the kernel) and which way it is currently held.
#[derive(Clone, Copy, Debug)]
pub struct Axis {
    min: i32,
    max: i32,
    held: i8,
}

impl Axis {
    pub fn new(min: i32, max: i32) -> Axis {
        Axis { min, max, held: 0 }
    }

    /// The new direction (-1, 0, 1) when it changed to a non-neutral one, else None. Past half the
    /// travel from the centre counts as pressed.
    fn update(&mut self, value: i32) -> Option<i8> {
        let (min, max) = (self.min as i64, self.max as i64);
        let span = (max - min) / 2;
        let dir = if span == 0 {
            0
        } else {
            let r = (value as i64 - (min + max) / 2) * 100 / span;
            if r < -50 {
                -1
            } else if r > 50 {
                1
            } else {
                0
            }
        };
        let changed = dir != self.held;
        self.held = dir;
        if changed && dir != 0 { Some(dir) } else { None }
    }
}

/// The state of one pad: its stick axes (ranges read from the kernel) and the hat.
pub struct Pad {
    x: Axis,
    y: Axis,
    hat_x: i32,
    hat_y: i32,
}

impl Pad {
    pub fn new(x: Axis, y: Axis) -> Pad {
        Pad { x, y, hat_x: 0, hat_y: 0 }
    }

    /// Feed one input event (type, code, value); the navigation key it produces, if any.
    pub fn event(&mut self, kind: u16, code: u16, value: i32) -> Option<Nav> {
        match (kind, code) {
            (EV_KEY, BTN_SOUTH | BTN_START) if value == 1 => Some(Nav::Select),
            (EV_KEY, BTN_EAST) if value == 1 => Some(Nav::Back),
            (EV_ABS, ABS_HAT0X) => {
                let was = std::mem::replace(&mut self.hat_x, value);
                if value != 0 && was == 0 { Some(if value < 0 { Nav::Left } else { Nav::Right }) } else { None }
            }
            (EV_ABS, ABS_HAT0Y) => {
                let was = std::mem::replace(&mut self.hat_y, value);
                if value != 0 && was == 0 { Some(if value < 0 { Nav::Up } else { Nav::Down }) } else { None }
            }
            (EV_ABS, ABS_X) => self.x.update(value).map(|d| if d < 0 { Nav::Left } else { Nav::Right }),
            (EV_ABS, ABS_Y) => self.y.update(value).map(|d| if d < 0 { Nav::Up } else { Nav::Down }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> Pad {
        Pad::new(Axis::new(0, 65535), Axis::new(0, 65535))
    }

    #[test]
    fn a_pad_is_told_from_a_keyboard_by_its_south_button() {
        // 8BitDo M30 over Bluetooth: KEY=3ff000000000000 0 800 0 0
        assert!(is_gamepad("3ff000000000000 0 800 0 0"));
        // Lite 2 over USB: KEY=7fff000000000000 0 0 0 0
        assert!(is_gamepad("7fff000000000000 0 0 0 0"));
        // A remote or keyboard: letters and KEY_ codes only.
        assert!(!is_gamepad("1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe"));
        assert!(!is_gamepad(""));
    }

    #[test]
    fn buttons_select_and_back() {
        let mut p = pad();
        assert_eq!(p.event(EV_KEY, BTN_SOUTH, 1), Some(Nav::Select));
        assert_eq!(p.event(EV_KEY, BTN_SOUTH, 0), None);
        assert_eq!(p.event(EV_KEY, BTN_START, 1), Some(Nav::Select));
        assert_eq!(p.event(EV_KEY, BTN_EAST, 1), Some(Nav::Back));
        assert_eq!(p.event(EV_KEY, 0x133, 1), None);
    }

    #[test]
    fn a_hat_dpad_navigates_once_per_press() {
        let mut p = pad();
        assert_eq!(p.event(EV_ABS, ABS_HAT0Y, -1), Some(Nav::Up));
        assert_eq!(p.event(EV_ABS, ABS_HAT0Y, 0), None);
        assert_eq!(p.event(EV_ABS, ABS_HAT0Y, 1), Some(Nav::Down));
        assert_eq!(p.event(EV_ABS, ABS_HAT0X, -1), Some(Nav::Left));
        assert_eq!(p.event(EV_ABS, ABS_HAT0X, 1), None, "swinging straight across is not a new press from rest");
        assert_eq!(p.event(EV_ABS, ABS_HAT0X, 0), None);
        assert_eq!(p.event(EV_ABS, ABS_HAT0X, 1), Some(Nav::Right));
    }

    #[test]
    fn a_dpad_on_the_left_stick_works_the_way_the_m30_reports_it() {
        // The M30 sends 0 / 32768 / 65535 on ABS_X and ABS_Y.
        let mut p = pad();
        assert_eq!(p.event(EV_ABS, ABS_Y, 0), Some(Nav::Up));
        assert_eq!(p.event(EV_ABS, ABS_Y, 0), None, "held: no repeat");
        assert_eq!(p.event(EV_ABS, ABS_Y, 32768), None);
        assert_eq!(p.event(EV_ABS, ABS_Y, 65535), Some(Nav::Down));
        assert_eq!(p.event(EV_ABS, ABS_Y, 32768), None);
        assert_eq!(p.event(EV_ABS, ABS_X, 65535), Some(Nav::Right));
        assert_eq!(p.event(EV_ABS, ABS_X, 32768), None);
        assert_eq!(p.event(EV_ABS, ABS_X, 0), Some(Nav::Left));
    }

    #[test]
    fn a_small_stick_movement_is_not_a_press_and_other_ranges_work() {
        let mut p = Pad::new(Axis::new(0, 255), Axis::new(-32768, 32767));
        assert_eq!(p.event(EV_ABS, ABS_X, 140), None, "drift near the centre");
        assert_eq!(p.event(EV_ABS, ABS_X, 255), Some(Nav::Right));
        assert_eq!(p.event(EV_ABS, ABS_Y, 5000), None);
        assert_eq!(p.event(EV_ABS, ABS_Y, -30000), Some(Nav::Up));
    }

    #[test]
    fn triggers_and_other_axes_do_nothing() {
        let mut p = pad();
        assert_eq!(p.event(EV_ABS, 2, 1023), None);
        assert_eq!(p.event(EV_ABS, 5, 1023), None);
    }
}
