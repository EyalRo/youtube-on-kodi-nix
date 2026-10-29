//! The real `bt::Adapter`: BlueZ over the system D-Bus (zbus, blocking).
//!
//! BlueZ exposes adapters and devices as objects under `/org/bluez`. We read them with
//! `ObjectManager.GetManagedObjects`, scan with `Adapter1.StartDiscovery`, and pair with
//! `Device1.Pair`. Pairing a game controller needs no PIN, but BlueZ still wants an agent to answer
//! its authorisation questions, so we register a `NoInputNoOutput` one that accepts.

use std::collections::HashMap;

use zbus::blocking::{fdo::ObjectManagerProxy, Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::bt::{Adapter, Device, Props};

const BLUEZ: &str = "org.bluez";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const AGENT_PATH: &str = "/org/appliance/agent";

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Answers BlueZ's pairing questions for a controller: accept everything, PIN "0000".
struct Agent;

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}
    fn request_pin_code(&self, _device: ObjectPath<'_>) -> String {
        "0000".into()
    }
    fn display_pin_code(&self, _device: ObjectPath<'_>, _pincode: String) {}
    fn request_passkey(&self, _device: ObjectPath<'_>) -> u32 {
        0
    }
    fn display_passkey(&self, _device: ObjectPath<'_>, _passkey: u32, _entered: u16) {}
    fn request_confirmation(&self, _device: ObjectPath<'_>, _passkey: u32) {}
    fn request_authorization(&self, _device: ObjectPath<'_>) {}
    fn authorize_service(&self, _device: ObjectPath<'_>, _uuid: String) {}
    fn cancel(&self) {}
}

pub struct Bluez {
    conn: Connection,
    agent_registered: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    // zbus errors read "org.bluez.Error.Failed: Page Timeout"; the part after the last ": " is the reason.
    let s = e.to_string();
    match s.rsplit_once(": ") {
        Some((_, reason)) if !reason.is_empty() => reason.to_string(),
        _ => s,
    }
}

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    props.get(key).and_then(|v| v.try_clone().ok()).and_then(|v| T::try_from(v).ok())
}

impl Bluez {
    pub fn connect() -> Result<Self, String> {
        Ok(Bluez { conn: Connection::system().map_err(err)?, agent_registered: false })
    }

    fn objects(&self) -> Result<Objects, String> {
        let om = ObjectManagerProxy::builder(&self.conn)
            .destination(BLUEZ)
            .map_err(err)?
            .path("/")
            .map_err(err)?
            .build()
            .map_err(err)?;
        let managed = om.get_managed_objects().map_err(err)?;
        Ok(managed
            .into_iter()
            .map(|(p, ifaces)| (p, ifaces.into_iter().map(|(i, props)| (i.to_string(), props)).collect()))
            .collect())
    }

    /// The first adapter, preferring `hci0`.
    fn adapter_path(&self) -> Result<Option<String>, String> {
        let mut paths: Vec<String> = self
            .objects()?
            .into_iter()
            .filter(|(_, i)| i.contains_key(ADAPTER_IFACE))
            .map(|(p, _)| p.to_string())
            .collect();
        paths.sort();
        Ok(paths.into_iter().next())
    }

    fn proxy(&self, path: &str, iface: &'static str) -> Result<Proxy<'static>, String> {
        Proxy::new(&self.conn, BLUEZ, path.to_string(), iface).map_err(err)
    }

    fn register_agent(&mut self) {
        if self.agent_registered {
            return;
        }
        let result = (|| -> Result<(), String> {
            self.conn.object_server().at(AGENT_PATH, Agent).map_err(err)?;
            let mgr = self.proxy("/org/bluez", "org.bluez.AgentManager1")?;
            let path = ObjectPath::try_from(AGENT_PATH).map_err(err)?;
            mgr.call_method("RegisterAgent", &(&path, "NoInputNoOutput")).map_err(err)?;
            mgr.call_method("RequestDefaultAgent", &(&path,)).map_err(err)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.agent_registered = true,
            Err(e) => tracing::warn!(error = %e, "cannot register the Bluetooth pairing agent"),
        }
    }
}

impl Adapter for Bluez {
    fn ready(&mut self) -> Result<bool, String> {
        let Some(path) = self.adapter_path()? else { return Ok(false) };
        let adapter = self.proxy(&path, ADAPTER_IFACE)?;
        if !adapter.get_property::<bool>("Powered").map_err(err)? {
            adapter.set_property("Powered", true).map_err(err)?;
        }
        self.register_agent();
        Ok(true)
    }

    fn set_scanning(&mut self, on: bool) -> Result<(), String> {
        let Some(path) = self.adapter_path()? else { return Err("no adapter".into()) };
        let adapter = self.proxy(&path, ADAPTER_IFACE)?;
        let r = adapter.call_method(if on { "StartDiscovery" } else { "StopDiscovery" }, &());
        match r {
            Ok(_) => Ok(()),
            // Already scanning / not scanning: the state we wanted.
            Err(e) if e.to_string().contains("InProgress") || e.to_string().contains("No discovery started") => Ok(()),
            Err(e) => Err(err(e)),
        }
    }

    fn devices(&mut self) -> Result<Vec<Device>, String> {
        let mut out = Vec::new();
        for (path, ifaces) in self.objects()? {
            let Some(p) = ifaces.get(DEVICE_IFACE) else { continue };
            let address: String = get(p, "Address").unwrap_or_default();
            let name: String = get::<String>(p, "Name").or_else(|| get(p, "Alias")).unwrap_or_default();
            // BlueZ's default alias for an unnamed device is its address with dashes; treat as no name.
            let name = if name.replace('-', ":") == address { String::new() } else { name };
            out.push(Device {
                path: path.to_string(),
                props: Props {
                    name,
                    address,
                    class: get(p, "Class"),
                    appearance: get(p, "Appearance"),
                    icon: get(p, "Icon"),
                    paired: get(p, "Paired").unwrap_or(false),
                    connected: get(p, "Connected").unwrap_or(false),
                    trusted: get(p, "Trusted").unwrap_or(false),
                },
            });
        }
        Ok(out)
    }

    fn pair_and_connect(&mut self, path: &str) -> Result<(), String> {
        self.register_agent();
        let dev = self.proxy(path, DEVICE_IFACE)?;
        if !dev.get_property::<bool>("Paired").map_err(err)? {
            dev.call_method("Pair", &()).map_err(err)?;
        }
        // Trusted devices may reconnect on their own (press Start/Home), which is the point.
        dev.set_property("Trusted", true).map_err(err)?;
        match dev.call_method("Connect", &()) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("AlreadyConnected") => Ok(()),
            Err(e) => Err(err(e)),
        }
    }

    fn connect(&mut self, path: &str) -> Result<(), String> {
        let dev = self.proxy(path, DEVICE_IFACE)?;
        match dev.call_method("Connect", &()) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("AlreadyConnected") => Ok(()),
            Err(e) => Err(err(e)),
        }
    }

    fn disconnect(&mut self, path: &str) -> Result<(), String> {
        self.proxy(path, DEVICE_IFACE)?.call_method("Disconnect", &()).map(|_| ()).map_err(err)
    }

    fn forget(&mut self, path: &str) -> Result<(), String> {
        let Some(adapter) = self.adapter_path()? else { return Err("no adapter".into()) };
        let dev = ObjectPath::try_from(path.to_string()).map_err(err)?;
        self.proxy(&adapter, ADAPTER_IFACE)?.call_method("RemoveDevice", &(&dev,)).map(|_| ()).map_err(err)
    }
}
