//! Bluetooth game-controller setup: what to show for each BlueZ device, and the pairing state
//! machine behind the Controllers page. This module has no D-Bus code (see `bluez.rs`), so all of
//! it is unit-tested against a fake adapter.

/// The controllers we draw and give pairing instructions for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Black 8BitDo M30, six-button Genesis-style pad.
    M30,
    /// Teal 8BitDo Lite 2.
    Lite2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Controller(Option<Model>),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Connected,
    Paired,
    Available,
}

/// The BlueZ `Device1` properties we look at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    pub name: String,
    pub address: String,
    pub class: Option<u32>,
    pub appearance: Option<u16>,
    pub icon: Option<String>,
    pub paired: bool,
    pub connected: bool,
    pub trusted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// The D-Bus object path, e.g. `/org/bluez/hci0/dev_E4_17_D8_00_00_01`.
    pub path: String,
    pub props: Props,
}

/// What the list shows for one device.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub path: String,
    pub name: String,
    pub kind: Kind,
    pub state: State,
}

const NAME_KEYWORDS: [&str; 9] = [
    "controller", "gamepad", "joy-con", "joycon", "dualshock", "dualsense", "xbox", "8bitdo", "game pad",
];

/// Decide whether a device is a game controller, and which of ours it is. Metadata is often
/// incomplete (a controller in pairing mode may advertise only a name), so any one signal is enough;
/// what matches nothing stays listed as `Other` instead of being hidden.
pub fn classify(p: &Props) -> Kind {
    let name = p.name.to_lowercase();
    if name.contains("8bitdo") {
        if name.contains("m30") {
            return Kind::Controller(Some(Model::M30));
        }
        if name.contains("lite 2") || name.contains("lite2") {
            return Kind::Controller(Some(Model::Lite2));
        }
        return Kind::Controller(None);
    }
    if p.icon.as_deref() == Some("input-gaming") {
        return Kind::Controller(None);
    }
    // Bluetooth appearance: 0x03C3 joystick, 0x03C4 gamepad.
    if matches!(p.appearance, Some(0x03C3 | 0x03C4)) {
        return Kind::Controller(None);
    }
    // Class of device: major class 5 (peripheral), minor 1 joystick / 2 gamepad.
    if let Some(c) = p.class {
        if (c >> 8) & 0x1f == 5 && matches!((c >> 2) & 0xf, 1 | 2) {
            return Kind::Controller(None);
        }
    }
    if NAME_KEYWORDS.iter().any(|k| name.contains(k)) {
        return Kind::Controller(None);
    }
    Kind::Other
}

pub fn state(p: &Props) -> State {
    if p.connected {
        State::Connected
    } else if p.paired {
        State::Paired
    } else {
        State::Available
    }
}

/// The list in display order: controllers first (connected, then paired, then available), then
/// everything else the scan found, each group by name.
pub fn views(devices: &[Device]) -> Vec<View> {
    let mut v: Vec<View> = devices
        .iter()
        .map(|d| View {
            path: d.path.clone(),
            name: if d.props.name.trim().is_empty() { d.props.address.clone() } else { d.props.name.clone() },
            kind: classify(&d.props),
            state: state(&d.props),
        })
        .collect();
    v.sort_by(|a, b| {
        let ka = matches!(a.kind, Kind::Other);
        let kb = matches!(b.kind, Kind::Other);
        ka.cmp(&kb)
            .then(a.state.cmp(&b.state))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    v
}

/// How to put a controller into pairing mode, from 8BitDo's manuals.
pub fn hint(model: Model) -> &'static str {
    match model {
        Model::M30 => "Hold X + Start to turn on, then hold the pair button on top for 2 seconds",
        Model::Lite2 => "Set the mode switch to D, press Home, then hold the pair button for 3 seconds",
    }
}

/// How to bring a paired controller back.
pub fn reconnect_hint(model: Model) -> &'static str {
    match model {
        Model::M30 => "Press Start",
        Model::Lite2 => "Press Home",
    }
}

pub fn model_title(model: Model) -> &'static str {
    match model {
        Model::M30 => "8BitDo M30 (black)",
        Model::Lite2 => "8BitDo Lite 2 (teal)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> Props {
        Props { name: name.into(), address: "E4:17:D8:00:00:01".into(), ..Props::default() }
    }

    #[test]
    fn our_two_controllers_are_recognised_by_their_advertised_names() {
        assert_eq!(classify(&p("8BitDo M30 gamepad")), Kind::Controller(Some(Model::M30)));
        assert_eq!(classify(&p("8Bitdo M30 Gamepad")), Kind::Controller(Some(Model::M30)));
        assert_eq!(classify(&p("8BitDo Lite 2")), Kind::Controller(Some(Model::Lite2)));
        assert_eq!(classify(&p("8BitDo Lite2")), Kind::Controller(Some(Model::Lite2)));
    }

    #[test]
    fn other_8bitdo_pads_are_controllers_without_a_model() {
        assert_eq!(classify(&p("8BitDo Pro 2")), Kind::Controller(None));
    }

    #[test]
    fn any_single_signal_marks_a_controller() {
        let mut x = p("");
        x.icon = Some("input-gaming".into());
        assert_eq!(classify(&x), Kind::Controller(None));
        let mut x = p("Thing");
        x.appearance = Some(0x03C4);
        assert_eq!(classify(&x), Kind::Controller(None));
        let mut x = p("Thing");
        x.appearance = Some(0x03C3);
        assert_eq!(classify(&x), Kind::Controller(None));
        // major 5 (peripheral), minor 2 (gamepad): 0x0508
        let mut x = p("Thing");
        x.class = Some(0x0508);
        assert_eq!(classify(&x), Kind::Controller(None));
        // minor 1 (joystick): 0x0504
        let mut x = p("Thing");
        x.class = Some(0x0504);
        assert_eq!(classify(&x), Kind::Controller(None));
        assert_eq!(classify(&p("Wireless Controller")), Kind::Controller(None));
        assert_eq!(classify(&p("Pro Controller")), Kind::Controller(None));
    }

    #[test]
    fn keyboards_mice_and_audio_are_other() {
        let mut kb = p("Some Keyboard");
        kb.class = Some(0x0540); // peripheral, keyboard
        assert_eq!(classify(&kb), Kind::Other);
        let mut audio = p("Headphones");
        audio.class = Some(0x240404);
        assert_eq!(classify(&audio), Kind::Other);
        assert_eq!(classify(&p("")), Kind::Other);
    }

    #[test]
    fn a_device_with_no_metadata_at_all_is_listed_not_hidden() {
        let d = Device { path: "/o/a".into(), props: Props { address: "AA:BB".into(), ..Props::default() } };
        let v = views(&[d]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "AA:BB");
        assert_eq!(v[0].kind, Kind::Other);
    }

    #[test]
    fn state_follows_connected_then_paired() {
        let mut x = p("a");
        assert_eq!(state(&x), State::Available);
        x.paired = true;
        assert_eq!(state(&x), State::Paired);
        x.connected = true;
        assert_eq!(state(&x), State::Connected);
        // connected without paired (some devices) still reads as connected
        let mut y = p("b");
        y.connected = true;
        assert_eq!(state(&y), State::Connected);
    }

    fn dev(path: &str, name: &str, paired: bool, connected: bool) -> Device {
        let mut props = p(name);
        props.paired = paired;
        props.connected = connected;
        Device { path: path.into(), props }
    }

    #[test]
    fn controllers_come_first_then_by_state_then_name() {
        let v = views(&[
            dev("/4", "Headphones", true, true),
            dev("/3", "8BitDo Lite 2", false, false),
            dev("/2", "8BitDo M30 gamepad", true, false),
            dev("/1", "Wireless Controller", true, true),
            dev("/5", "Aaa keyboard", false, false),
        ]);
        let paths: Vec<_> = v.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, ["/1", "/2", "/3", "/4", "/5"]);
    }

    #[test]
    fn hints_name_the_buttons_from_the_official_manuals() {
        assert!(hint(Model::M30).contains("X + Start") && hint(Model::M30).contains("2 seconds"));
        assert!(hint(Model::Lite2).contains("mode switch to D") && hint(Model::Lite2).contains("3 seconds"));
        assert_eq!(reconnect_hint(Model::M30), "Press Start");
        assert_eq!(reconnect_hint(Model::Lite2), "Press Home");
    }
}

// ---------------------------------------------------------------------------------------------
// The Controllers page's state machine. It talks to BlueZ through `Adapter`, so it is tested with
// a fake; `bluez.rs` is the real one.

pub trait Adapter {
    /// Whether there is a Bluetooth adapter, powered on (it is powered on if it was off).
    fn ready(&mut self) -> Result<bool, String>;
    fn set_scanning(&mut self, on: bool) -> Result<(), String>;
    fn devices(&mut self) -> Result<Vec<Device>, String>;
    /// Pair, mark trusted (so it reconnects by itself later), then connect.
    fn pair_and_connect(&mut self, path: &str) -> Result<(), String>;
    fn connect(&mut self, path: &str) -> Result<(), String>;
    fn disconnect(&mut self, path: &str) -> Result<(), String>;
    /// Remove the device and its pairing.
    fn forget(&mut self, path: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Open,
    Close,
    ToggleScan,
    /// Refresh the list (the worker sends it about once a second while the page is open).
    Poll,
    Pair(String),
    Connect(String),
    Disconnect(String),
    Forget(String),
}

/// What the page shows after a command.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    /// False when there is no usable adapter; the page then says so instead of listing.
    pub available: bool,
    pub scanning: bool,
    pub views: Vec<View>,
    /// One line of feedback: the result of the last action or a problem.
    pub message: String,
}

/// A scan that finds nothing more stops by itself after this many polls (about 1 s each).
const SCAN_POLLS: u32 = 60;

pub struct Session<A: Adapter> {
    adapter: A,
    open: bool,
    available: bool,
    scanning: bool,
    scan_polls: u32,
    message: String,
    last: Vec<View>,
}

impl<A: Adapter> Session<A> {
    pub fn new(adapter: A) -> Self {
        Session { adapter, open: false, available: false, scanning: false, scan_polls: 0, message: String::new(), last: Vec::new() }
    }

    fn name_of(&self, path: &str) -> String {
        self.last.iter().find(|v| v.path == path).map(|v| v.name.clone()).unwrap_or_else(|| "controller".into())
    }

    fn stop_scan(&mut self) {
        if self.scanning {
            if let Err(e) = self.adapter.set_scanning(false) {
                tracing::warn!(error = %e, "cannot stop the Bluetooth scan");
            }
            self.scanning = false;
            self.scan_polls = 0;
        }
    }

    fn refresh(&mut self) {
        match self.adapter.devices() {
            Ok(d) => self.last = views(&d),
            Err(e) => {
                tracing::warn!(error = %e, "cannot list Bluetooth devices");
                self.message = format!("Bluetooth error: {e}");
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { available: self.available, scanning: self.scanning, views: self.last.clone(), message: self.message.clone() }
    }

    /// An action on one device. Scanning stops first: BlueZ pairs more reliably while idle.
    fn act(&mut self, path: &str, done: &str, verb: &str, f: impl FnOnce(&mut A, &str) -> Result<(), String>) {
        self.stop_scan();
        let name = self.name_of(path);
        match f(&mut self.adapter, path) {
            Ok(()) => self.message = format!("{done}: {name}"),
            Err(e) => {
                tracing::warn!(%path, error = %e, action = verb, "Bluetooth action failed");
                self.message = format!("Could not {verb}: {name} ({e})");
            }
        }
        self.refresh();
    }

    pub fn handle(&mut self, cmd: Command) -> Snapshot {
        match cmd {
            Command::Open => {
                self.open = true;
                self.message.clear();
                match self.adapter.ready() {
                    Ok(true) => {
                        self.available = true;
                        self.refresh();
                    }
                    Ok(false) => {
                        self.available = false;
                        self.last.clear();
                        self.message = "No Bluetooth adapter found".into();
                    }
                    Err(e) => {
                        self.available = false;
                        self.last.clear();
                        self.message = format!("Bluetooth is not available ({e})");
                    }
                }
            }
            Command::Close => {
                self.open = false;
                self.stop_scan();
            }
            Command::ToggleScan if self.available => {
                if self.scanning {
                    self.stop_scan();
                    self.message = "Scan stopped".into();
                } else {
                    match self.adapter.set_scanning(true) {
                        Ok(()) => {
                            self.scanning = true;
                            self.scan_polls = 0;
                            self.message = "Scanning for controllers".into();
                        }
                        Err(e) => self.message = format!("Could not start the scan ({e})"),
                    }
                }
                self.refresh();
            }
            Command::Poll if self.available && self.open => {
                if self.scanning {
                    self.scan_polls += 1;
                    if self.scan_polls >= SCAN_POLLS {
                        self.stop_scan();
                        self.message = "Scan finished".into();
                    }
                }
                self.refresh();
            }
            Command::Pair(p) if self.available => self.act(&p, "Connected", "connect", |a, p| a.pair_and_connect(p)),
            Command::Connect(p) if self.available => self.act(&p, "Connected", "connect", |a, p| a.connect(p)),
            Command::Disconnect(p) if self.available => self.act(&p, "Disconnected", "disconnect", |a, p| a.disconnect(p)),
            Command::Forget(p) if self.available => self.act(&p, "Forgot", "forget", |a, p| a.forget(p)),
            // Not open or no adapter: nothing to do.
            _ => {}
        }
        self.snapshot()
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A fake adapter: records calls, holds a device list, can fail on demand.
    #[derive(Default)]
    struct Fake {
        calls: Rc<RefCell<Vec<String>>>,
        present: bool,
        devices: Vec<Device>,
        fail: Option<String>,
    }

    impl Fake {
        fn new(devices: Vec<Device>) -> (Self, Rc<RefCell<Vec<String>>>) {
            let calls = Rc::new(RefCell::new(Vec::new()));
            (Fake { calls: calls.clone(), present: true, devices, fail: None }, calls)
        }
        fn log(&self, s: String) {
            self.calls.borrow_mut().push(s);
        }
        fn result(&self) -> Result<(), String> {
            match &self.fail {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
        fn set(&mut self, path: &str, f: impl FnOnce(&mut Props)) {
            if let Some(d) = self.devices.iter_mut().find(|d| d.path == path) {
                f(&mut d.props);
            }
        }
    }

    impl Adapter for Fake {
        fn ready(&mut self) -> Result<bool, String> {
            self.log("ready".into());
            Ok(self.present)
        }
        fn set_scanning(&mut self, on: bool) -> Result<(), String> {
            self.log(format!("scan {on}"));
            self.result()
        }
        fn devices(&mut self) -> Result<Vec<Device>, String> {
            Ok(self.devices.clone())
        }
        fn pair_and_connect(&mut self, path: &str) -> Result<(), String> {
            self.log(format!("pair {path}"));
            self.result()?;
            self.set(path, |p| {
                p.paired = true;
                p.trusted = true;
                p.connected = true;
            });
            Ok(())
        }
        fn connect(&mut self, path: &str) -> Result<(), String> {
            self.log(format!("connect {path}"));
            self.result()?;
            self.set(path, |p| p.connected = true);
            Ok(())
        }
        fn disconnect(&mut self, path: &str) -> Result<(), String> {
            self.log(format!("disconnect {path}"));
            self.result()?;
            self.set(path, |p| p.connected = false);
            Ok(())
        }
        fn forget(&mut self, path: &str) -> Result<(), String> {
            self.log(format!("forget {path}"));
            self.result()?;
            self.devices.retain(|d| d.path != path);
            Ok(())
        }
    }

    fn pad(path: &str, name: &str) -> Device {
        Device { path: path.into(), props: Props { name: name.into(), address: "AA".into(), ..Props::default() } }
    }

    #[test]
    fn opening_lists_known_devices_without_scanning() {
        let (f, calls) = Fake::new(vec![pad("/a", "8BitDo Lite 2")]);
        let mut s = Session::new(f);
        let snap = s.handle(Command::Open);
        assert!(snap.available && !snap.scanning);
        assert_eq!(snap.views.len(), 1);
        assert_eq!(*calls.borrow(), ["ready"]);
    }

    #[test]
    fn no_adapter_says_so_and_ignores_actions() {
        let (mut f, calls) = Fake::new(vec![]);
        f.present = false;
        let mut s = Session::new(f);
        let snap = s.handle(Command::Open);
        assert!(!snap.available);
        assert_eq!(snap.message, "No Bluetooth adapter found");
        s.handle(Command::ToggleScan);
        s.handle(Command::Pair("/a".into()));
        assert_eq!(*calls.borrow(), ["ready"]);
    }

    #[test]
    fn scan_toggles_on_and_off_and_closing_stops_it() {
        let (f, calls) = Fake::new(vec![]);
        let mut s = Session::new(f);
        s.handle(Command::Open);
        assert!(s.handle(Command::ToggleScan).scanning);
        assert!(!s.handle(Command::ToggleScan).scanning);
        s.handle(Command::ToggleScan);
        let snap = s.handle(Command::Close);
        assert!(!snap.scanning);
        assert_eq!(*calls.borrow(), ["ready", "scan true", "scan false", "scan true", "scan false"]);
    }

    #[test]
    fn a_scan_ends_by_itself_after_a_minute_of_polling() {
        let (f, _) = Fake::new(vec![]);
        let mut s = Session::new(f);
        s.handle(Command::Open);
        s.handle(Command::ToggleScan);
        let mut snap = Snapshot::default();
        for _ in 0..SCAN_POLLS {
            snap = s.handle(Command::Poll);
        }
        assert!(!snap.scanning);
        assert_eq!(snap.message, "Scan finished");
    }

    #[test]
    fn pairing_stops_the_scan_first_then_pairs_and_reports() {
        let (f, calls) = Fake::new(vec![pad("/a", "8BitDo M30 gamepad")]);
        let mut s = Session::new(f);
        s.handle(Command::Open);
        s.handle(Command::ToggleScan);
        let snap = s.handle(Command::Pair("/a".into()));
        assert!(!snap.scanning);
        assert_eq!(snap.message, "Connected: 8BitDo M30 gamepad");
        assert_eq!(snap.views[0].state, State::Connected);
        assert_eq!(*calls.borrow(), ["ready", "scan true", "scan false", "pair /a"]);
    }

    #[test]
    fn a_failed_pairing_reports_why_and_keeps_listing() {
        let (mut f, _) = Fake::new(vec![pad("/a", "8BitDo Lite 2")]);
        f.fail = Some("Authentication Failed".into());
        let mut s = Session::new(f);
        s.handle(Command::Open);
        let snap = s.handle(Command::Pair("/a".into()));
        assert_eq!(snap.message, "Could not connect: 8BitDo Lite 2 (Authentication Failed)");
        assert_eq!(snap.views.len(), 1);
        assert_eq!(snap.views[0].state, State::Available);
    }

    #[test]
    fn connect_disconnect_and_forget_update_the_list() {
        let mut d = pad("/a", "8BitDo Lite 2");
        d.props.paired = true;
        let (f, _) = Fake::new(vec![d]);
        let mut s = Session::new(f);
        s.handle(Command::Open);
        assert_eq!(s.handle(Command::Connect("/a".into())).views[0].state, State::Connected);
        let snap = s.handle(Command::Disconnect("/a".into()));
        assert_eq!(snap.views[0].state, State::Paired);
        assert_eq!(snap.message, "Disconnected: 8BitDo Lite 2");
        let snap = s.handle(Command::Forget("/a".into()));
        assert!(snap.views.is_empty());
        assert_eq!(snap.message, "Forgot: 8BitDo Lite 2");
    }

    #[test]
    fn poll_before_open_does_nothing() {
        let (f, calls) = Fake::new(vec![]);
        let mut s = Session::new(f);
        s.handle(Command::Poll);
        assert!(calls.borrow().is_empty());
    }
}
