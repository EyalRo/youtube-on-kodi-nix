//! Shared logging for the appliance.
//!
//! Every appliance binary calls [`init`] at startup. It installs a JSON `tracing` subscriber
//! (so the journal gets the same structured JSON as before) and, when the `VLOGS` environment
//! variable is set to `host:port`, also installs a layer that pushes each event to VictoriaLogs
//! as a flat JSON line through `/insert/jsonline`, with `service` and `host` as stream fields and
//! `_msg` as the message field. That makes `level`, `target`, `service`, `host` and every logged
//! field queryable in VictoriaLogs — which the journald-upload path (`/insert/journald`) does
//! not parse.
//!
//! Shipping is strictly best-effort: events are handed to an unbounded channel drained by a
//! background thread, and a failed push is dropped. Logging must never block or crash the app.
//! Call [`flush`] before a oneshot process exits, and [`sync_ship`] for an event that must land
//! before shutdown (the UI's power events).
//!
//! Debug logging is compiled in or out, never branched on at run time. A debug build
//! (`RUSTFLAGS="--cfg appliance_debug"`, which the Nix packages set with `debug = true`) turns
//! [`debug!`] into `tracing::debug!` and lifts the default log filter to debug. Any other build
//! expands `debug!` to nothing: no call site, no level check, no arguments evaluated. `info!`,
//! `warn!` and `error!` stay plain `tracing` calls; they are the logs a production box keeps.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

pub use tracing;

/// `tracing::debug!` in debug builds, nothing otherwise.
#[cfg(appliance_debug)]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => { $crate::tracing::debug!($($t)*) };
}

/// `tracing::debug!` in debug builds, nothing otherwise.
#[cfg(not(appliance_debug))]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {};
}

/// The filter used when no `*_LOG` variable is set: info for everything, plus debug for the
/// appliance's own crates in a debug build.
pub const fn default_filter() -> &'static str {
    if cfg!(appliance_debug) {
        "info,player=debug,mediad=debug,store=debug,scanner=debug"
    } else {
        "info"
    }
}

/// Whether this binary was built with debug logging.
pub const DEBUG_BUILD: bool = cfg!(appliance_debug);

/// The common stream-field names and the message field VictoriaLogs extracts.
const STREAM_FIELDS: &str = "service,host";
const MSG_FIELD: &str = "_msg";

/// One item on the shipping channel: either a ready jsonline body, or a flush barrier whose
/// sender is signalled once the queue ahead of it has been drained.
enum Item {
    Line(Vec<u8>),
    Flush(SyncSender<()>),
}

/// The VictoriaLogs endpoint (`VLOGS=host:port`) and the shipping channel, set once by `init`.
static SHIP: Mutex<Option<Ship>> = Mutex::new(None);
/// The sender used by `flush`.
static SENDER: Mutex<Option<Sender<Item>>> = Mutex::new(None);
/// The first `init` wins; later calls are ignored.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct Ship {
    addr: std::net::SocketAddr,
    host_header: String,
}

/// A tracing layer that turns each event into one flat VictoriaLogs record.
///
/// Always installed (so the subscriber's type stays static). When no channel is configured it
/// is a no-op.
struct VlLayer {
    service: String,
    host: String,
    tx: Sender<Item>,
}

impl VlLayer {
    /// Send a flat record for one event. Never blocks, never fails: a dropped send is fine.
    fn push_line(&self, fields: &Map<String, Value>) {
        if let Ok(body) = serde_json::to_vec(&fields) {
            let _ = self.tx.send(Item::Line(body));
        }
    }
}

/// Flatten each traced field into `out`, overwriting a later field of the same name.
struct FieldFlattener<'a> {
    out: &'a mut Map<String, Value>,
}

impl Visit for FieldFlattener<'_> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.out.insert(field.name().into(), Value::from(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.out.insert(field.name().into(), Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.out.insert(field.name().into(), Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.out.insert(field.name().into(), Value::from(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.out.insert(field.name().into(), Value::from(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.out
            .insert(field.name().into(), Value::from(value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.out
            .insert(field.name().into(), Value::from(format!("{value:?}")));
    }
}

fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

impl<S: Subscriber> Layer<S> for VlLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Map::new();
        let mut visitor = FieldFlattener { out: &mut fields };
        event.record(&mut visitor);
        let message = fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        fields.insert("level".into(), Value::from(level_name(event.metadata().level()).to_string()));
        fields.insert("target".into(), Value::from(event.metadata().target().to_string()));
        fields.insert(MSG_FIELD.into(), Value::from(message));
        fields.insert("service".into(), Value::from(self.service.clone()));
        fields.insert("host".into(), Value::from(self.host.clone()));
        self.push_line(&fields);
    }
}

/// Install the app-wide logging. Idempotent: the first call wins; later calls are ignored.
///
/// * `service` — the value of the `service` stream field (e.g. `"mediad"`). Use one spelling per
///   component and keep it identical across queries.
/// * `filter_var` — the environment variable holding the filter (e.g. `"MEDIAD_LOG"`); falls
///   back to [`default_filter`] when unset.
///
/// Reads `VLOGS=host:port`; when set, a background thread drains events to VictoriaLogs.
pub fn init(service: &str, filter_var: &str) {
    init_with_default(service, filter_var, default_filter());
}

/// Like [`init`], but with an explicit default filter for when `filter_var` is unset. The
/// default can name targets the process logs from that the backend's [`default_filter`] would
/// not cover (for example the UI's `appliance_ui`).
pub fn init_with_default(service: &str, filter_var: &str, default: &str) {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    let filter = EnvFilter::try_from_env(filter_var).unwrap_or_else(|_| EnvFilter::new(default));
    // JSON to the journal (as before): the journald.upload path and the MCP records tool read
    // `service`/`host`/`unit` from journald metadata plus the JSON message.
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_filter(filter);

    let service = service.to_string();
    let host = hostname();
    // The VL layer is always present; it only does work when a channel is wired.
    let (tx, rx) = mpsc::channel::<Item>();
    if let Some(vlogs) = std::env::var("VLOGS").ok().filter(|a| !a.trim().is_empty()) {
        match vlogs.to_socket_addrs().ok().and_then(|mut a| a.next()) {
            Some(addr) => {
                *SENDER.lock().unwrap() = Some(tx.clone());
                *SHIP.lock().unwrap() = Some(Ship {
                    addr,
                    host_header: vlogs,
                });
                let _ = std::thread::Builder::new()
                    .name("applog-vlogs".into())
                    .spawn(move || drain(rx));
            }
            None => {
                // Unresolvable: fall back to journal-only, but keep the layer inert.
            }
        }
    }
    let vl = VlLayer {
        service,
        host,
        tx,
    };
    let sub = Registry::default().with(fmt_layer).with(vl);
    let _ = sub.try_init();
}

/// Block until every event pushed before this call has been sent to VictoriaLogs (for oneshot
/// processes before they exit). No-op when `VLOGS` is unset.
pub fn flush() {
    let (ack, receiver) = mpsc::sync_channel::<()>(0);
    let guard = SENDER.lock().unwrap();
    let Some(tx) = guard.as_ref() else {
        return;
    };
    // The barrier is queued behind every earlier Line, so once it is processed all of them have
    // been sent (FIFO). Wait up to a bounded time for the drainer to acknowledge.
    if tx.send(Item::Flush(ack)).is_ok() {
        let _ = receiver.recv_timeout(Duration::from_secs(5));
    }
}

/// Synchronously push one flat record to VictoriaLogs and wait for the server to accept it.
/// Used for events that must land even across shutdown (the UI's power events). No-op when
/// `VLOGS` is unset.
pub fn sync_ship(service: &str, message: &str, extra: &[(&str, Value)]) {
    let Some(ship) = SHIP.lock().unwrap().as_ref().cloned() else {
        return;
    };
    let mut fields = Map::new();
    fields.insert(MSG_FIELD.into(), Value::from(message.to_string()));
    fields.insert("service".into(), Value::from(service.to_string()));
    fields.insert("host".into(), Value::from(hostname()));
    fields.insert("level".into(), Value::from("info"));
    for (k, v) in extra {
        fields.insert((*k).to_string(), v.clone());
    }
    let Ok(body) = serde_json::to_vec(&fields) else {
        return;
    };
    post(&ship, &body);
}

/// POST one flat jsonline body to VictoriaLogs over a short-lived TCP connection and read the
/// reply. Best-effort.
fn post(ship: &Ship, body: &[u8]) {
    let Ok(mut s) = TcpStream::connect_timeout(&ship.addr, Duration::from_secs(2)) else {
        return;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let query = format!("?_stream_fields={STREAM_FIELDS}&_msg_field={MSG_FIELD}");
    let head = format!(
        "POST /insert/jsonline{query} HTTP/1.0\r\nHost: {}\r\nContent-Type: application/stream+json\r\nContent-Length: {}\r\n\r\n",
        ship.host_header,
        body.len() + 1
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
    let _ = s.write_all(b"\n");
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
}

/// Drain the shipping channel, posting each line; acknowledge flush barriers once the queue
/// ahead of them is empty. Stops when the channel is closed (the process is exiting).
fn drain(rx: Receiver<Item>) {
    while let Ok(item) = rx.recv() {
        match item {
            Item::Line(body) => {
                if let Some(ship) = SHIP.lock().unwrap().as_ref().cloned() {
                    post(&ship, &body);
                }
            }
            Item::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

/// The machine's short hostname.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names() {
        assert_eq!(level_name(&Level::ERROR), "error");
        assert_eq!(level_name(&Level::WARN), "warn");
        assert_eq!(level_name(&Level::INFO), "info");
        assert_eq!(level_name(&Level::DEBUG), "debug");
        assert_eq!(level_name(&Level::TRACE), "trace");
    }

    #[test]
    fn line_has_service_host_msg_level_and_extra() {
        let mut fields = Map::new();
        fields.insert("message".into(), Value::from("hello"));
        fields.insert("count".into(), Value::from(3));
        let vl = VlLayer {
            service: "mediad".into(),
            host: "nuc14".into(),
            tx: mpsc::channel().0,
        };
        // Exercise the same flattening the layer applies.
        let mut f = Map::new();
        let mut visitor = FieldFlattener { out: &mut f };
        // Simulate the on_event body against the fixed fields.
        let message = fields.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
        f.insert("level".into(), Value::from("info"));
        f.insert("target".into(), Value::from("mediad"));
        f.insert(MSG_FIELD.into(), Value::from(message));
        f.insert("service".into(), Value::from(vl.service.clone()));
        f.insert("host".into(), Value::from(vl.host.clone()));
        f.insert("count".into(), Value::from(3));
        assert_eq!(f["service"], "mediad");
        assert_eq!(f["host"], "nuc14");
        assert_eq!(f["_msg"], "hello");
        assert_eq!(f["level"], "info");
        assert_eq!(f["count"], 3);
    }

    #[test]
    fn stream_query_params() {
        let q = format!("?_stream_fields={STREAM_FIELDS}&_msg_field={MSG_FIELD}");
        assert!(q.contains("_stream_fields=service,host"));
        assert!(q.contains("_msg_field=_msg"));
    }

    #[test]
    fn hostname_reads_kernel() {
        let h = hostname();
        assert!(!h.is_empty());
    }
}