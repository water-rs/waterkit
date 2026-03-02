use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use zbus::Connection;
use zbus::zvariant::{Array, OwnedValue, Value};

const NEARD_SERVICE: &str = "org.neard";
const OBJECT_MANAGER_PATH: &str = "/";
const ADAPTER_IFACE: &str = "org.neard.Adapter";
const TAG_IFACE: &str = "org.neard.Tag";
const RECORD_IFACE: &str = "org.neard.Record";

type OwnedPropertyMap = HashMap<String, OwnedValue>;
type ManagedObjects = HashMap<
    zbus::zvariant::OwnedObjectPath,
    HashMap<zbus::names::OwnedInterfaceName, OwnedPropertyMap>,
>;

fn prop_string(props: &OwnedPropertyMap, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok())
        .map(str::to_owned)
}

fn prop_object_paths(props: &OwnedPropertyMap, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|value| <&Array<'_>>::try_from(value).ok())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.downcast_ref::<zbus::zvariant::ObjectPath<'_>>()
                        .ok()
                        .map(|path| path.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_hex_id(raw: &str) -> Option<Vec<u8>> {
    let compact: String = raw.chars().filter(|ch| *ch != ':' && *ch != '-').collect();
    if compact.is_empty() || !compact.len().is_multiple_of(2) {
        return None;
    }
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

fn decode_tag_type(tag_type: &str) -> NfcTagType {
    let lowered = tag_type.to_ascii_lowercase();
    if lowered.contains("mifare") {
        return NfcTagType::MifareClassic;
    }
    if lowered.contains("type 1") || lowered.contains("type1") {
        return NfcTagType::Type1;
    }
    if lowered.contains("type 2") || lowered.contains("type2") {
        return NfcTagType::Type2;
    }
    if lowered.contains("type 3") || lowered.contains("type3") {
        return NfcTagType::Type3;
    }
    if lowered.contains("type 4") || lowered.contains("type4") {
        return NfcTagType::Type4;
    }
    if lowered.contains("type 5") || lowered.contains("type5") {
        return NfcTagType::Type5;
    }
    NfcTagType::Unknown
}

fn decode_uri_payload(payload: &[u8]) -> Option<String> {
    let (&prefix, rest) = payload.split_first()?;
    let prefix_str = match prefix {
        0x00 => "",
        0x01 => "http://www.",
        0x02 => "https://www.",
        0x03 => "http://",
        0x04 => "https://",
        _ => "",
    };
    let suffix = std::str::from_utf8(rest).ok()?;
    Some(format!("{prefix_str}{suffix}"))
}

fn decode_text_payload(payload: &[u8]) -> Option<String> {
    let (&status, rest) = payload.split_first()?;
    let lang_len = usize::from(status & 0x3F);
    if rest.len() < lang_len {
        return None;
    }
    std::str::from_utf8(&rest[lang_len..])
        .ok()
        .map(std::string::ToString::to_string)
}

fn parse_record(props: &OwnedPropertyMap) -> Option<NdefRecord> {
    let record_type = prop_string(props, "Type")?;
    match record_type.as_str() {
        "Text" => {
            let text = prop_string(props, "Representation")?;
            Some(NdefRecord::text(&text))
        }
        "URI" => {
            let uri = prop_string(props, "URI")?;
            Some(NdefRecord::uri(&uri))
        }
        _ => None,
    }
}

fn parse_tag(path: &str, objects: &ManagedObjects) -> Option<NfcTag> {
    let interfaces = objects
        .iter()
        .find(|(object_path, _)| object_path.to_string() == path)
        .map(|(_, ifaces)| ifaces)?;
    let tag_props = interfaces.get(TAG_IFACE)?;

    let id = prop_string(tag_props, "UID")
        .and_then(|raw| parse_hex_id(&raw))
        .unwrap_or_default();
    let tag_type = prop_string(tag_props, "Type")
        .map(|raw| decode_tag_type(&raw))
        .unwrap_or(NfcTagType::Unknown);

    let mut records = Vec::new();
    for record_path in prop_object_paths(tag_props, "Records") {
        if let Some(record_interfaces) = objects
            .iter()
            .find(|(object_path, _)| object_path.to_string() == record_path)
            .map(|(_, ifaces)| ifaces)
            && let Some(record_props) = record_interfaces.get(RECORD_IFACE)
            && let Some(record) = parse_record(record_props)
        {
            records.push(record);
        }
    }

    let ndef_message = if records.is_empty() {
        None
    } else {
        Some(NdefMessage { records })
    };
    Some(NfcTag {
        id,
        tag_type,
        ndef_message,
    })
}

fn dbus_string_value(value: &str) -> Result<OwnedValue, NfcError> {
    Value::from(value).try_into().map_err(|error| {
        NfcError::PlatformError(format!("encode D-Bus string value failed: {error}"))
    })
}

fn encode_neard_records(
    message: &NdefMessage,
) -> Result<Vec<HashMap<&'static str, OwnedValue>>, NfcError> {
    let mut records = Vec::with_capacity(message.records.len());
    for record in &message.records {
        if record.record_type.as_slice() == b"T" {
            let text = decode_text_payload(&record.payload)
                .ok_or_else(|| NfcError::WriteFailed("invalid text record payload".into()))?;
            let mut encoded = HashMap::new();
            encoded.insert("Type", dbus_string_value("Text")?);
            encoded.insert("Encoding", dbus_string_value("UTF-8")?);
            encoded.insert("Language", dbus_string_value("en")?);
            encoded.insert("Representation", dbus_string_value(&text)?);
            records.push(encoded);
            continue;
        }

        if record.record_type.as_slice() == b"U" {
            let uri = decode_uri_payload(&record.payload)
                .ok_or_else(|| NfcError::WriteFailed("invalid URI record payload".into()))?;
            let mut encoded = HashMap::new();
            encoded.insert("Type", dbus_string_value("URI")?);
            encoded.insert("URI", dbus_string_value(&uri)?);
            records.push(encoded);
            continue;
        }

        return Err(NfcError::WriteFailed(
            "linux backend supports text/uri NDEF records only".into(),
        ));
    }
    Ok(records)
}

async fn get_system_connection() -> Result<Connection, NfcError> {
    Connection::system()
        .await
        .map_err(|error| NfcError::PlatformError(format!("connect system bus failed: {error}")))
}

async fn has_neard_owner(conn: &Connection) -> Result<bool, NfcError> {
    let proxy = zbus::fdo::DBusProxy::builder(conn)
        .build()
        .await
        .map_err(|error| NfcError::PlatformError(format!("build DBus proxy failed: {error}")))?;
    proxy
        .name_has_owner(NEARD_SERVICE.try_into().expect("valid neard service name"))
        .await
        .map_err(|error| NfcError::PlatformError(format!("query neard owner failed: {error}")))
}

async fn first_adapter_path(conn: &Connection) -> Result<String, NfcError> {
    let object_manager = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination(NEARD_SERVICE)
        .map_err(|error| NfcError::PlatformError(format!("set destination failed: {error}")))?
        .path(OBJECT_MANAGER_PATH)
        .map_err(|error| {
            NfcError::PlatformError(format!("set object manager path failed: {error}"))
        })?
        .build()
        .await
        .map_err(|error| {
            NfcError::PlatformError(format!("build object manager proxy failed: {error}"))
        })?;
    let objects = object_manager
        .get_managed_objects()
        .await
        .map_err(|error| {
            NfcError::PlatformError(format!("list managed objects failed: {error}"))
        })?;
    for (path, ifaces) in objects {
        if ifaces.contains_key(ADAPTER_IFACE) {
            return Ok(path.to_string());
        }
    }
    Err(NfcError::NotAvailable)
}

async fn start_poll_loop(conn: &Connection, adapter_path: &str) -> Result<(), NfcError> {
    let adapter_proxy = zbus::Proxy::new(conn, NEARD_SERVICE, adapter_path, ADAPTER_IFACE)
        .await
        .map_err(|error| NfcError::PlatformError(format!("build adapter proxy failed: {error}")))?;
    adapter_proxy
        .call_method("StartPollLoop", &("Initiator",))
        .await
        .map_err(|error| NfcError::PlatformError(format!("start poll loop failed: {error}")))?;
    Ok(())
}

async fn stop_poll_loop(conn: &Connection, adapter_path: &str) -> Result<(), NfcError> {
    let adapter_proxy = zbus::Proxy::new(conn, NEARD_SERVICE, adapter_path, ADAPTER_IFACE)
        .await
        .map_err(|error| NfcError::PlatformError(format!("build adapter proxy failed: {error}")))?;
    adapter_proxy
        .call_method("StopPollLoop", &())
        .await
        .map_err(|error| NfcError::PlatformError(format!("stop poll loop failed: {error}")))?;
    Ok(())
}

fn run_tag_listener(
    adapter_path: String,
    tag_tx: async_channel::Sender<NfcTag>,
    latest_tag_path: Arc<Mutex<Option<String>>>,
    stop_rx: async_channel::Receiver<()>,
    stopped: Arc<AtomicBool>,
) {
    futures::executor::block_on(async move {
        let conn = match get_system_connection().await {
            Ok(connection) => connection,
            Err(_) => return,
        };
        if !has_neard_owner(&conn).await.unwrap_or(false) {
            return;
        }

        let object_manager = match zbus::fdo::ObjectManagerProxy::builder(&conn)
            .destination(NEARD_SERVICE)
            .and_then(|builder| builder.path(OBJECT_MANAGER_PATH))
        {
            Ok(builder) => match builder.build().await {
                Ok(proxy) => proxy,
                Err(_) => return,
            },
            Err(_) => return,
        };

        if let Ok(objects) = object_manager.get_managed_objects().await {
            for (path, ifaces) in &objects {
                let path_str = path.to_string();
                if path_str.starts_with(&adapter_path)
                    && ifaces.contains_key(TAG_IFACE)
                    && let Some(tag) = parse_tag(&path_str, &objects)
                {
                    if let Ok(mut guard) = latest_tag_path.lock() {
                        *guard = Some(path_str.clone());
                    }
                    if tag_tx.try_send(tag).is_err() {
                        return;
                    }
                }
            }
        }

        let mut added_stream = match object_manager.receive_interfaces_added().await {
            Ok(stream) => stream,
            Err(_) => return,
        };
        loop {
            let signal_next = added_stream.next();
            let stop_next = stop_rx.recv();
            futures::pin_mut!(signal_next, stop_next);
            match futures::future::select(stop_next, signal_next).await {
                futures::future::Either::Left(_) => break,
                futures::future::Either::Right((Some(signal), _)) => {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(args) = signal.args() else {
                        continue;
                    };
                    let path = args.object_path().to_string();
                    if !path.starts_with(&adapter_path) {
                        continue;
                    }
                    let interfaces = args.interfaces_and_properties();
                    if !interfaces.contains_key(TAG_IFACE) {
                        continue;
                    }
                    let Ok(objects) = object_manager.get_managed_objects().await else {
                        continue;
                    };
                    if let Some(tag) = parse_tag(&path, &objects) {
                        if let Ok(mut guard) = latest_tag_path.lock() {
                            *guard = Some(path);
                        }
                        if tag_tx.try_send(tag).is_err() {
                            break;
                        }
                    }
                }
                futures::future::Either::Right((None, _)) => break,
            }
        }
    });
}

pub fn nfc_is_available() -> bool {
    futures::executor::block_on(async {
        let Ok(conn) = get_system_connection().await else {
            return false;
        };
        has_neard_owner(&conn).await.unwrap_or(false)
    })
}

#[derive(Debug)]
pub struct NfcReaderInner {
    adapter_path: String,
    latest_tag_path: Arc<Mutex<Option<String>>>,
    stop_tx: async_channel::Sender<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stopped: Arc<AtomicBool>,
}

impl NfcReaderInner {
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        let conn = get_system_connection().await?;
        if !has_neard_owner(&conn).await? {
            return Err(NfcError::NotAvailable);
        }

        let adapter_path = first_adapter_path(&conn).await?;
        start_poll_loop(&conn, &adapter_path).await?;

        let (tag_tx, tag_rx) = async_channel::bounded(32);
        let (stop_tx, stop_rx) = async_channel::bounded(1);
        let latest_tag_path = Arc::new(Mutex::new(None));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_latest_tag_path = Arc::clone(&latest_tag_path);
        let worker_stopped = Arc::clone(&stopped);
        let worker_adapter_path = adapter_path.clone();
        let worker = std::thread::Builder::new()
            .name("waterkit-nfc-linux".into())
            .spawn(move || {
                run_tag_listener(
                    worker_adapter_path,
                    tag_tx,
                    worker_latest_tag_path,
                    stop_rx,
                    worker_stopped,
                );
            })
            .map_err(|error| NfcError::PlatformError(format!("spawn listener failed: {error}")))?;

        Ok((
            Self {
                adapter_path,
                latest_tag_path,
                stop_tx,
                worker: Mutex::new(Some(worker)),
                stopped,
            },
            tag_rx,
        ))
    }

    pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
        let tag_path = self
            .latest_tag_path
            .lock()
            .map_err(|error| {
                NfcError::PlatformError(format!("latest tag mutex poisoned: {error}"))
            })?
            .clone()
            .ok_or_else(|| NfcError::WriteFailed("no NFC tag discovered yet".into()))?;

        let records = encode_neard_records(&message)?;
        let conn = get_system_connection().await?;
        let proxy = zbus::Proxy::new(&conn, NEARD_SERVICE, tag_path, TAG_IFACE)
            .await
            .map_err(|error| NfcError::PlatformError(format!("build tag proxy failed: {error}")))?;
        proxy
            .call_method("Write", &(records,))
            .await
            .map_err(|error| NfcError::WriteFailed(format!("tag write failed: {error}")))?;
        Ok(())
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::Relaxed) {
            return;
        }

        let _ = self.stop_tx.try_send(());
        let adapter_path = self.adapter_path.clone();
        std::thread::spawn(move || {
            futures::executor::block_on(async move {
                let Ok(conn) = get_system_connection().await else {
                    return;
                };
                let _ = stop_poll_loop(&conn, &adapter_path).await;
            });
        });

        if let Ok(mut guard) = self.worker.lock()
            && let Some(worker) = guard.take()
        {
            std::thread::spawn(move || {
                let _ = worker.join();
            });
        }
    }
}

impl Drop for NfcReaderInner {
    fn drop(&mut self) {
        self.stop();
    }
}
