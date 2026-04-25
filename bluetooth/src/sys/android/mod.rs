use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use futures::channel::oneshot;
use futures::future;
use jni::objects::{GlobalRef, JByteArray, JClass, JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::collections::{BTreeMap, HashMap};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();
static CALLBACK_NATIVES_REGISTERED: OnceLock<()> = OnceLock::new();
static SCAN_CALLBACKS: OnceLock<Mutex<BTreeMap<i64, ScanCallbackState>>> = OnceLock::new();
static GATT_CALLBACKS: OnceLock<Mutex<BTreeMap<i64, Arc<GattCallbackState>>>> = OnceLock::new();
static CLASSIC_DISCOVERY_CALLBACKS: OnceLock<
    Mutex<BTreeMap<i64, async_channel::Sender<ClassicDevice>>>,
> = OnceLock::new();
static NEXT_CALLBACK_STATE_ID: AtomicI64 = AtomicI64::new(1);
const HELPER_CLASS_NAME: &str = "waterkit.bluetooth.BluetoothHelper";
const BOND_BONDED: i32 = 12;

#[derive(Debug, Clone)]
struct ScanCallbackState {
    sender: async_channel::Sender<ScanResult>,
    name_prefix: Option<String>,
}

#[derive(Debug)]
struct BleScanSession {
    scanner: GlobalRef,
    callback: GlobalRef,
    callback_state_id: i64,
}

#[derive(Debug)]
struct ClassicDiscoverySession {
    callback: GlobalRef,
    callback_state_id: i64,
}

enum SppCommand {
    Read {
        max_bytes: usize,
        tx: oneshot::Sender<Result<Vec<u8>, BluetoothError>>,
    },
    Write {
        data: Vec<u8>,
        tx: oneshot::Sender<Result<usize, BluetoothError>>,
    },
    Close {
        tx: oneshot::Sender<()>,
    },
}

#[derive(Debug)]
struct GattCallbackState {
    connect: Mutex<Option<oneshot::Sender<Result<(), BluetoothError>>>>,
    discover_services: Mutex<Option<oneshot::Sender<Result<Vec<GattService>, BluetoothError>>>>,
    reads: Mutex<BTreeMap<(String, String), oneshot::Sender<Result<Vec<u8>, BluetoothError>>>>,
    writes: Mutex<BTreeMap<(String, String), oneshot::Sender<Result<(), BluetoothError>>>>,
    subscriptions: Mutex<BTreeMap<(String, String), async_channel::Sender<Vec<u8>>>>,
}

impl GattCallbackState {
    fn new() -> Self {
        Self {
            connect: Mutex::new(None),
            discover_services: Mutex::new(None),
            reads: Mutex::new(BTreeMap::new()),
            writes: Mutex::new(BTreeMap::new()),
            subscriptions: Mutex::new(BTreeMap::new()),
        }
    }
}

fn scan_callbacks() -> &'static Mutex<BTreeMap<i64, ScanCallbackState>> {
    SCAN_CALLBACKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn gatt_callbacks() -> &'static Mutex<BTreeMap<i64, Arc<GattCallbackState>>> {
    GATT_CALLBACKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn classic_discovery_callbacks()
-> &'static Mutex<BTreeMap<i64, async_channel::Sender<ClassicDevice>>> {
    CLASSIC_DISCOVERY_CALLBACKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn next_callback_state_id() -> Result<i64, BluetoothError> {
    let id = NEXT_CALLBACK_STATE_ID.fetch_add(1, Ordering::Relaxed);
    if id <= 0 {
        return Err(BluetoothError::PlatformError(format!(
            "invalid callback state id generated: {id}"
        )));
    }
    Ok(id)
}

fn with_android_context<T, F>(f: F) -> Result<T, BluetoothError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, BluetoothError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| BluetoothError::PlatformError(format!("JavaVM::from_raw: {e}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| BluetoothError::PlatformError(format!("attach_current_thread: {e}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-bluetooth: ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), BluetoothError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getCacheDir: {e}")))?;
    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getAbsolutePath: {e}")))?;
    let dex_path = format!(
        "{}/waterkit_bluetooth.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| BluetoothError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| BluetoothError::PlatformError(format!("to_str: {e}")))?
    );
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| BluetoothError::PlatformError(format!("write DEX: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| BluetoothError::PlatformError(format!("metadata DEX: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| BluetoothError::PlatformError(format!("set_permissions DEX: {e}")))?;
    }
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| BluetoothError::PlatformError(format!("new_string: {e}")))?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getClassLoader: {e}")))?;
    let dex_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| BluetoothError::PlatformError(format!("find_class: {e}")))?;
    let loader = env
        .new_object(
            dex_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| BluetoothError::PlatformError(format!("new_object: {e}")))?;
    let global = env
        .new_global_ref(loader)
        .map_err(|e| BluetoothError::PlatformError(format!("global_ref: {e}")))?;
    if CLASS_LOADER.set(global).is_err() {
        assert!(
            CLASS_LOADER.get().is_some(),
            "waterkit-bluetooth: class loader initialization race left loader unset"
        );
    }
    Ok(())
}

fn load_class<'local>(
    env: &mut JNIEnv<'local>,
    class_name: &str,
) -> Result<JClass<'local>, BluetoothError> {
    let class_name = env
        .new_string(class_name)
        .map_err(|e| BluetoothError::PlatformError(format!("new_string class_name: {e}")))?;
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| BluetoothError::PlatformError("Class loader not initialized".into()))?;
    let class = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("loadClass: {e}")))?;
    Ok(class.into())
}

fn register_callback_natives(env: &mut JNIEnv) -> Result<(), BluetoothError> {
    if CALLBACK_NATIVES_REGISTERED.get().is_some() {
        return Ok(());
    }

    let scan_callback_class = load_class(env, "waterkit.bluetooth.BleScanBridgeCallback")?;
    let scan_natives = [jni::NativeMethod {
        name: "onScanResultNative".into(),
        sig: "(Ljava/lang/String;Ljava/lang/String;I[Ljava/lang/String;)V".into(),
        fn_ptr: Java_waterkit_bluetooth_BleScanBridgeCallback_onScanResultNative as *mut _,
    }];
    env.register_native_methods(scan_callback_class, &scan_natives)
        .map_err(|e| {
            BluetoothError::PlatformError(format!(
                "register_native_methods BleScanBridgeCallback failed: {e}"
            ))
        })?;

    let gatt_callback_class = load_class(env, "waterkit.bluetooth.BleGattBridgeCallback")?;
    let gatt_natives = [
        jni::NativeMethod {
            name: "onConnectionStateNative".into(),
            sig: "(Ljava/lang/String;ZI)V".into(),
            fn_ptr: Java_waterkit_bluetooth_BleGattBridgeCallback_onConnectionStateNative as *mut _,
        },
        jni::NativeMethod {
            name: "onServicesDiscoveredNative".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: Java_waterkit_bluetooth_BleGattBridgeCallback_onServicesDiscoveredNative
                as *mut _,
        },
        jni::NativeMethod {
            name: "onCharacteristicReadNative".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[BI)V".into(),
            fn_ptr: Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicReadNative
                as *mut _,
        },
        jni::NativeMethod {
            name: "onCharacteristicWriteNative".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicWriteNative
                as *mut _,
        },
        jni::NativeMethod {
            name: "onCharacteristicChangedNative".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V".into(),
            fn_ptr: Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicChangedNative
                as *mut _,
        },
    ];
    env.register_native_methods(gatt_callback_class, &gatt_natives)
        .map_err(|e| {
            BluetoothError::PlatformError(format!(
                "register_native_methods BleGattBridgeCallback failed: {e}"
            ))
        })?;

    let classic_callback_class =
        load_class(env, "waterkit.bluetooth.ClassicDiscoveryBridgeCallback")?;
    let classic_natives = [jni::NativeMethod {
        name: "onDeviceFoundNative".into(),
        sig: "(Ljava/lang/String;Ljava/lang/String;IZ)V".into(),
        fn_ptr: Java_waterkit_bluetooth_ClassicDiscoveryBridgeCallback_onDeviceFoundNative
            as *mut _,
    }];
    env.register_native_methods(classic_callback_class, &classic_natives)
        .map_err(|e| {
            BluetoothError::PlatformError(format!(
                "register_native_methods ClassicDiscoveryBridgeCallback failed: {e}"
            ))
        })?;

    let _ = CALLBACK_NATIVES_REGISTERED.set(());
    Ok(())
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    future::ready(with_android_context(jni_api::get_adapter_state)).await
}

fn get_helper_class<'local>(
    env: &mut JNIEnv<'local>,
) -> Result<jni::objects::JClass<'local>, BluetoothError> {
    load_class(env, HELPER_CLASS_NAME)
}

fn get_paired_devices_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
) -> Result<Vec<ClassicDevice>, BluetoothError> {
    init_dex(env, context)?;
    let helper_class = get_helper_class(env)?;
    let paired_obj = env
        .call_static_method(
            &helper_class,
            "getPairedDevices",
            "(Landroid/content/Context;)[Landroid/bluetooth/BluetoothDevice;",
            &[JValue::Object(context)],
        )
        .map_err(|e| BluetoothError::PlatformError(format!("getPairedDevices: {e}")))?
        .l()
        .map_err(|e| BluetoothError::PlatformError(format!("pairedDevices return: {e}")))?;

    if paired_obj.is_null() {
        return Err(BluetoothError::PlatformError(
            "BluetoothHelper.getPairedDevices returned null".into(),
        ));
    }

    let paired = JObjectArray::from(paired_obj);
    let count = env
        .get_array_length(&paired)
        .map_err(|e| BluetoothError::PlatformError(format!("get_array_length: {e}")))?;
    let mut devices = Vec::with_capacity(count as usize);

    for index in 0..count {
        let device_obj = env
            .get_object_array_element(&paired, index)
            .map_err(|e| BluetoothError::PlatformError(format!("get_object_array_element: {e}")))?;
        if device_obj.is_null() {
            continue;
        }

        let address_obj = env
            .call_method(&device_obj, "getAddress", "()Ljava/lang/String;", &[])
            .map_err(|e| BluetoothError::PlatformError(format!("BluetoothDevice.getAddress: {e}")))?
            .l()
            .map_err(|e| {
                BluetoothError::PlatformError(format!(
                    "BluetoothDevice.getAddress return decode: {e}"
                ))
            })?;
        if address_obj.is_null() {
            return Err(BluetoothError::PlatformError(
                "BluetoothDevice.getAddress returned null".into(),
            ));
        }
        let address: String = env
            .get_string(&JString::from(address_obj))
            .map_err(|e| {
                BluetoothError::PlatformError(format!("BluetoothDevice.getAddress get_string: {e}"))
            })?
            .into();

        let name_obj = env
            .call_method(&device_obj, "getName", "()Ljava/lang/String;", &[])
            .map_err(|e| BluetoothError::PlatformError(format!("BluetoothDevice.getName: {e}")))?
            .l()
            .map_err(|e| {
                BluetoothError::PlatformError(format!("BluetoothDevice.getName return decode: {e}"))
            })?;
        let name = if name_obj.is_null() {
            None
        } else {
            let value: String = env
                .get_string(&JString::from(name_obj))
                .map_err(|e| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothDevice.getName get_string: {e}"
                    ))
                })?
                .into();
            if value.is_empty() { None } else { Some(value) }
        };

        let class_obj = env
            .call_method(
                &device_obj,
                "getBluetoothClass",
                "()Landroid/bluetooth/BluetoothClass;",
                &[],
            )
            .map_err(|e| {
                BluetoothError::PlatformError(format!("BluetoothDevice.getBluetoothClass: {e}"))
            })?
            .l()
            .map_err(|e| {
                BluetoothError::PlatformError(format!(
                    "BluetoothDevice.getBluetoothClass return decode: {e}"
                ))
            })?;
        let device_class = if class_obj.is_null() {
            0
        } else {
            env.call_method(&class_obj, "getMajorDeviceClass", "()I", &[])
                .map_err(|e| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothClass.getMajorDeviceClass: {e}"
                    ))
                })?
                .i()
                .map_err(|e| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothClass.getMajorDeviceClass return decode: {e}"
                    ))
                })? as u32
        };

        let bond_state = env
            .call_method(&device_obj, "getBondState", "()I", &[])
            .map_err(|e| {
                BluetoothError::PlatformError(format!("BluetoothDevice.getBondState: {e}"))
            })?
            .i()
            .map_err(|e| {
                BluetoothError::PlatformError(format!(
                    "BluetoothDevice.getBondState return decode: {e}"
                ))
            })?;

        devices.push(ClassicDevice {
            device: BluetoothDevice {
                id: DeviceId(address),
                name,
                rssi: None,
                is_connected: false,
            },
            device_class,
            is_paired: bond_state == BOND_BONDED,
        });
    }

    Ok(devices)
}

pub struct BleScannerInner {
    session: Mutex<Option<BleScanSession>>,
}

impl std::fmt::Debug for BleScannerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleScannerInner").finish()
    }
}

impl BleScannerInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        match adapter_state().await? {
            AdapterState::PoweredOn => Ok(Self {
                session: Mutex::new(None),
            }),
            AdapterState::PoweredOff => Err(BluetoothError::PoweredOff),
            AdapterState::Unavailable | AdapterState::Unknown => Err(BluetoothError::NotAvailable),
            AdapterState::Unauthorized => Err(BluetoothError::PermissionDenied),
        }
    }

    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        {
            let session = self.session.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("BLE scan session mutex poisoned: {error}"))
            })?;
            if session.is_some() {
                return Err(BluetoothError::PlatformError(
                    "BLE scan already active on this scanner".into(),
                ));
            }
        }

        let callback_state_id = next_callback_state_id()?;
        let (tx, rx) = async_channel::unbounded();
        let scan_state = ScanCallbackState {
            sender: tx,
            name_prefix: filter.name_prefix.clone(),
        };
        let service_uuids: Vec<String> = filter
            .service_uuids
            .iter()
            .map(|uuid| uuid.0.clone())
            .collect();

        let session = with_android_context(|env, context| {
            init_dex(env, context)?;
            register_callback_natives(env)?;
            let helper_class = get_helper_class(env)?;
            let callback_class = load_class(env, "waterkit.bluetooth.BleScanBridgeCallback")?;
            let callback = env
                .new_object(callback_class, "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "new BleScanBridgeCallback failed: {error}"
                    ))
                })?;
            env.set_field(
                &callback,
                "waterkit_scan_state",
                "J",
                JValue::Long(callback_state_id),
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "set BleScanBridgeCallback.waterkit_scan_state failed: {error}"
                ))
            })?;

            let service_uuid_array = if service_uuids.is_empty() {
                JObject::null()
            } else {
                let string_class = env.find_class("java/lang/String").map_err(|error| {
                    BluetoothError::PlatformError(format!("find java/lang/String failed: {error}"))
                })?;
                let array = env
                    .new_object_array(
                        i32::try_from(service_uuids.len()).map_err(|_| {
                            BluetoothError::PlatformError(format!(
                                "service UUID filter count exceeds i32: {}",
                                service_uuids.len()
                            ))
                        })?,
                        string_class,
                        JObject::null(),
                    )
                    .map_err(|error| {
                        BluetoothError::PlatformError(format!(
                            "new service UUID object array failed: {error}"
                        ))
                    })?;

                for (index, uuid) in service_uuids.iter().enumerate() {
                    let value = env.new_string(uuid).map_err(|error| {
                        BluetoothError::PlatformError(format!("new_string UUID failed: {error}"))
                    })?;
                    env.set_object_array_element(
                        &array,
                        i32::try_from(index).map_err(|_| {
                            BluetoothError::PlatformError(format!(
                                "service UUID index exceeds i32: {index}"
                            ))
                        })?,
                        &value,
                    )
                    .map_err(|error| {
                        BluetoothError::PlatformError(format!(
                            "set service UUID array element failed: {error}"
                        ))
                    })?;
                }
                JObject::from(array)
            };

            let scanner = env
                .call_static_method(
                    helper_class,
                    "startBleScan",
                    "(Landroid/content/Context;[Ljava/lang/String;Lwaterkit/bluetooth/BleScanCallback;)Landroid/bluetooth/le/BluetoothLeScanner;",
                    &[
                        JValue::Object(context),
                        JValue::Object(&service_uuid_array),
                        JValue::Object(&callback),
                    ],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothHelper.startBleScan failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothHelper.startBleScan return decode failed: {error}"
                    ))
                })?;

            if scanner.is_null() {
                return Err(BluetoothError::NotAvailable);
            }

            let scanner = env.new_global_ref(scanner).map_err(|error| {
                BluetoothError::PlatformError(format!("new_global_ref scanner failed: {error}"))
            })?;
            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::PlatformError(format!("new_global_ref callback failed: {error}"))
            })?;

            Ok(BleScanSession {
                scanner,
                callback,
                callback_state_id,
            })
        })?;

        {
            let mut callbacks = scan_callbacks().lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BLE scan callback registry mutex poisoned: {error}"
                ))
            })?;
            callbacks.insert(callback_state_id, scan_state);
        }

        let mut stored_session = self.session.lock().map_err(|error| {
            BluetoothError::PlatformError(format!("BLE scan session mutex poisoned: {error}"))
        })?;
        *stored_session = Some(session);
        Ok(rx)
    }

    pub fn stop_scan(&self) {
        let session = self
            .session
            .lock()
            .ok()
            .and_then(|mut session| session.take());
        let Some(session) = session else {
            return;
        };

        if let Ok(mut callbacks) = scan_callbacks().lock() {
            callbacks.remove(&session.callback_state_id);
        }

        with_android_context(|env, context| {
            init_dex(env, context)?;
            let helper_class = get_helper_class(env)?;
            env.call_static_method(
                helper_class,
                "stopBleScan",
                "(Landroid/bluetooth/le/BluetoothLeScanner;Landroid/bluetooth/le/ScanCallback;)V",
                &[
                    JValue::Object(session.scanner.as_obj()),
                    JValue::Object(session.callback.as_obj()),
                ],
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothHelper.stopBleScan failed: {error}"
                ))
            })?;
            Ok(())
        })
        .unwrap_or_else(|error| panic!("waterkit-bluetooth: stop_scan failed: {error}"));
    }
}

impl Drop for BleScannerInner {
    fn drop(&mut self) {
        self.stop_scan();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleScanBridgeCallback_onScanResultNative(
    mut env: JNIEnv,
    callback: JObject,
    device_address: JString,
    device_name: JObject,
    rssi: jni::sys::jint,
    service_uuids: JObjectArray,
) {
    let callback_state_id = env
        .get_field(&callback, "waterkit_scan_state", "J")
        .unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: read waterkit_scan_state failed in scan callback: {error}")
        })
        .j()
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode waterkit_scan_state failed in scan callback: {error}"
            )
        });

    assert!(
        callback_state_id > 0,
        "waterkit-bluetooth: invalid waterkit_scan_state in scan callback: {callback_state_id}"
    );

    let state = {
        let callbacks = scan_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: scan callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: scan callback state missing for id {callback_state_id}"
        );
        return;
    };

    let device_address: String = env
        .get_string(&device_address)
        .unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode deviceAddress failed in scan callback: {error}")
        })
        .into();

    let device_name = if device_name.is_null() {
        None
    } else {
        let value: String = env
            .get_string(&JString::from(device_name))
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: decode deviceName failed in scan callback: {error}")
            })
            .into();
        if value.is_empty() { None } else { Some(value) }
    };

    if let Some(prefix) = state.name_prefix.as_ref()
        && !device_name
            .as_deref()
            .is_some_and(|name| name.starts_with(prefix))
    {
        return;
    }

    let service_uuids_len = env
        .get_array_length(&service_uuids)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: get service UUID array length failed in scan callback: {error}"
            )
        });
    let mut parsed_uuids = Vec::with_capacity(service_uuids_len as usize);
    for index in 0..service_uuids_len {
        let value = env
            .get_object_array_element(&service_uuids, index)
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-bluetooth: get service UUID array element failed at {index}: {error}"
                )
            });
        if value.is_null() {
            continue;
        }
        let value: String = env
            .get_string(&JString::from(value))
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: decode service UUID failed at {index}: {error}")
            })
            .into();
        if !value.is_empty() {
            parsed_uuids.push(Uuid(value));
        }
    }

    if let Err(error) = state.sender.try_send(ScanResult {
        device: BluetoothDevice {
            id: DeviceId(device_address),
            name: device_name,
            rssi: i16::try_from(rssi).ok(),
            is_connected: false,
        },
        service_uuids: parsed_uuids,
        manufacturer_data: HashMap::new(),
    }) {
        debug_assert!(
            false,
            "waterkit-bluetooth: dropping scan result because receiver is closed: {error}"
        );
    }
}

fn parse_services_payload(payload: &str) -> Result<Vec<GattService>, BluetoothError> {
    let mut services = Vec::new();
    for service in payload.split(';').filter(|segment| !segment.is_empty()) {
        let mut service_parts = service.splitn(3, ':');
        let service_uuid = service_parts
            .next()
            .ok_or_else(|| BluetoothError::GattError("missing service UUID in payload".into()))?;
        let is_primary = service_parts.next().ok_or_else(|| {
            BluetoothError::GattError("missing service primary flag in payload".into())
        })? == "1";
        let characteristics_payload = service_parts.next().unwrap_or_default();

        let mut characteristics = Vec::new();
        for characteristic in characteristics_payload
            .split(',')
            .filter(|segment| !segment.is_empty())
        {
            let parts: Vec<&str> = characteristic.split(':').collect();
            if parts.len() != 6 {
                return Err(BluetoothError::GattError(format!(
                    "invalid characteristic payload shape: {characteristic}"
                )));
            }
            characteristics.push(GattCharacteristic {
                uuid: Uuid(parts[0].to_string()),
                properties: CharacteristicProperties {
                    read: parts[1] == "1",
                    write: parts[2] == "1",
                    write_without_response: parts[3] == "1",
                    notify: parts[4] == "1",
                    indicate: parts[5] == "1",
                },
            });
        }

        services.push(GattService {
            uuid: Uuid(service_uuid.to_string()),
            is_primary,
            characteristics,
        });
    }
    Ok(services)
}

fn callback_state_id(env: &mut JNIEnv, callback: &JObject, field: &str) -> i64 {
    let value = env.get_field(callback, field, "J").unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: read {field} failed in callback: {error}")
    });
    let state_id = value.j().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: decode {field} failed in callback: {error}")
    });
    assert!(
        state_id > 0,
        "waterkit-bluetooth: invalid callback state id in {field}: {state_id}"
    );
    state_id
}

fn jni_uuid_from_string<'local>(
    env: &mut JNIEnv<'local>,
    uuid: &str,
) -> Result<JObject<'local>, BluetoothError> {
    let uuid_class = env.find_class("java/util/UUID").map_err(|error| {
        BluetoothError::PlatformError(format!("find java/util/UUID failed: {error}"))
    })?;
    let uuid = env.new_string(uuid).map_err(|error| {
        BluetoothError::PlatformError(format!("new_string UUID failed: {error}"))
    })?;
    env.call_static_method(
        uuid_class,
        "fromString",
        "(Ljava/lang/String;)Ljava/util/UUID;",
        &[JValue::Object(&uuid)],
    )
    .map_err(|error| BluetoothError::PlatformError(format!("UUID.fromString failed: {error}")))?
    .l()
    .map_err(|error| {
        BluetoothError::PlatformError(format!("UUID.fromString decode failed: {error}"))
    })
}

fn jni_characteristic<'local>(
    env: &mut JNIEnv<'local>,
    gatt: &JObject<'local>,
    service_uuid: &str,
    characteristic_uuid: &str,
) -> Result<JObject<'local>, BluetoothError> {
    let service_uuid_obj = jni_uuid_from_string(env, service_uuid)?;
    let service = env
        .call_method(
            gatt,
            "getService",
            "(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattService;",
            &[JValue::Object(&service_uuid_obj)],
        )
        .map_err(|error| {
            BluetoothError::PlatformError(format!("BluetoothGatt.getService failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            BluetoothError::PlatformError(format!(
                "BluetoothGatt.getService decode failed: {error}"
            ))
        })?;
    if service.is_null() {
        return Err(BluetoothError::GattError(format!(
            "service not found: {service_uuid}"
        )));
    }

    let characteristic_uuid_obj = jni_uuid_from_string(env, characteristic_uuid)?;
    let characteristic = env
        .call_method(
            &service,
            "getCharacteristic",
            "(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattCharacteristic;",
            &[JValue::Object(&characteristic_uuid_obj)],
        )
        .map_err(|error| {
            BluetoothError::PlatformError(format!(
                "BluetoothGattService.getCharacteristic failed: {error}"
            ))
        })?
        .l()
        .map_err(|error| {
            BluetoothError::PlatformError(format!(
                "BluetoothGattService.getCharacteristic decode failed: {error}"
            ))
        })?;
    if characteristic.is_null() {
        return Err(BluetoothError::GattError(format!(
            "characteristic not found: service={service_uuid}, characteristic={characteristic_uuid}"
        )));
    }

    Ok(characteristic)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onConnectionStateNative(
    mut env: JNIEnv,
    callback: JObject,
    _device_address: JString,
    connected: jni::sys::jboolean,
    status: jni::sys::jint,
) {
    let callback_state_id = callback_state_id(&mut env, &callback, "waterkit_gatt_state");
    let state = {
        let callbacks = gatt_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing GATT callback state for id {callback_state_id}"
        );
        return;
    };

    let mut connect_slot = state.connect.lock().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: GATT connect slot mutex poisoned: {error}")
    });
    if let Some(tx) = connect_slot.take() {
        let result = if connected != 0 && status == 0 {
            Ok(())
        } else {
            Err(BluetoothError::ConnectionFailed(format!(
                "onConnectionStateChange failed: connected={}, status={status}",
                connected != 0
            )))
        };
        let _ = tx.send(result);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onServicesDiscoveredNative(
    mut env: JNIEnv,
    callback: JObject,
    _device_address: JString,
    payload: JString,
    status: jni::sys::jint,
) {
    let callback_state_id = callback_state_id(&mut env, &callback, "waterkit_gatt_state");
    let state = {
        let callbacks = gatt_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing GATT callback state for id {callback_state_id}"
        );
        return;
    };

    let result = if status == 0 {
        let payload: String = env
            .get_string(&payload)
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: decode services payload failed in callback: {error}")
            })
            .into();
        parse_services_payload(&payload)
    } else {
        Err(BluetoothError::GattError(format!(
            "onServicesDiscovered failed with status {status}"
        )))
    };

    let mut discover_slot = state.discover_services.lock().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: GATT discover slot mutex poisoned: {error}")
    });
    if let Some(tx) = discover_slot.take() {
        let _ = tx.send(result);
    } else {
        debug_assert!(
            false,
            "waterkit-bluetooth: discover callback fired without pending receiver"
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicReadNative(
    mut env: JNIEnv,
    callback: JObject,
    _device_address: JString,
    service_uuid: JString,
    characteristic_uuid: JString,
    value: JByteArray,
    status: jni::sys::jint,
) {
    let callback_state_id = callback_state_id(&mut env, &callback, "waterkit_gatt_state");
    let state = {
        let callbacks = gatt_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing GATT callback state for id {callback_state_id}"
        );
        return;
    };

    let service_uuid: String = env
        .get_string(&service_uuid)
        .unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode service UUID failed in read callback: {error}")
        })
        .into();
    let characteristic_uuid: String = env
        .get_string(&characteristic_uuid)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in read callback: {error}"
            )
        })
        .into();
    let key = (service_uuid, characteristic_uuid);

    let result = if status == 0 {
        env.convert_byte_array(value).map_err(|error| {
            BluetoothError::GattError(format!("decode read value byte array failed: {error}"))
        })
    } else {
        Err(BluetoothError::GattError(format!(
            "onCharacteristicRead failed with status {status}"
        )))
    };

    let mut reads = state.reads.lock().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: GATT read map mutex poisoned: {error}")
    });
    if let Some(tx) = reads.remove(&key) {
        let _ = tx.send(result);
    } else {
        debug_assert!(
            false,
            "waterkit-bluetooth: read callback fired without pending receiver"
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicWriteNative(
    mut env: JNIEnv,
    callback: JObject,
    _device_address: JString,
    service_uuid: JString,
    characteristic_uuid: JString,
    status: jni::sys::jint,
) {
    let callback_state_id = callback_state_id(&mut env, &callback, "waterkit_gatt_state");
    let state = {
        let callbacks = gatt_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing GATT callback state for id {callback_state_id}"
        );
        return;
    };

    let service_uuid: String = env
        .get_string(&service_uuid)
        .unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode service UUID failed in write callback: {error}")
        })
        .into();
    let characteristic_uuid: String = env
        .get_string(&characteristic_uuid)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in write callback: {error}"
            )
        })
        .into();
    let key = (service_uuid, characteristic_uuid);

    let result = if status == 0 {
        Ok(())
    } else {
        Err(BluetoothError::GattError(format!(
            "onCharacteristicWrite failed with status {status}"
        )))
    };

    let mut writes = state.writes.lock().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: GATT write map mutex poisoned: {error}")
    });
    if let Some(tx) = writes.remove(&key) {
        let _ = tx.send(result);
    } else {
        debug_assert!(
            false,
            "waterkit-bluetooth: write callback fired without pending receiver"
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicChangedNative(
    mut env: JNIEnv,
    callback: JObject,
    _device_address: JString,
    service_uuid: JString,
    characteristic_uuid: JString,
    value: JByteArray,
) {
    let callback_state_id = callback_state_id(&mut env, &callback, "waterkit_gatt_state");
    let state = {
        let callbacks = gatt_callbacks().lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT callback registry mutex poisoned: {error}")
        });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(state) = state else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing GATT callback state for id {callback_state_id}"
        );
        return;
    };

    let service_uuid: String = env
        .get_string(&service_uuid)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode service UUID failed in notification callback: {error}"
            )
        })
        .into();
    let characteristic_uuid: String = env
        .get_string(&characteristic_uuid)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in notification callback: {error}"
            )
        })
        .into();
    let key = (service_uuid, characteristic_uuid);
    let payload = env.convert_byte_array(value).unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: decode notification payload failed: {error}")
    });

    let sender = {
        let subscriptions = state.subscriptions.lock().unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: GATT subscription map mutex poisoned: {error}")
        });
        subscriptions.get(&key).cloned()
    };
    if let Some(sender) = sender {
        if let Err(error) = sender.try_send(payload) {
            debug_assert!(
                false,
                "waterkit-bluetooth: dropping notification payload because receiver is closed: {error}"
            );
        }
    } else {
        debug_assert!(
            false,
            "waterkit-bluetooth: notification callback fired without subscriber"
        );
    }
}

pub struct BleConnectionInner {
    gatt: GlobalRef,
    callback: GlobalRef,
    callback_state_id: i64,
}

impl std::fmt::Debug for BleConnectionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleConnectionInner")
            .field("callback_state_id", &self.callback_state_id)
            .finish()
    }
}

impl BleConnectionInner {
    fn callback_state(&self) -> Result<Arc<GattCallbackState>, BluetoothError> {
        gatt_callbacks()
            .lock()
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT callback registry mutex poisoned: {error}"
                ))
            })?
            .get(&self.callback_state_id)
            .cloned()
            .ok_or_else(|| {
                BluetoothError::PlatformError(format!(
                    "missing GATT callback state for id {}",
                    self.callback_state_id
                ))
            })
    }

    fn close_gatt(&self) -> Result<(), BluetoothError> {
        with_android_context(|env, _context| {
            env.call_method(self.gatt.as_obj(), "disconnect", "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGatt.disconnect failed: {error}"
                    ))
                })?;
            env.call_method(self.gatt.as_obj(), "close", "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!("BluetoothGatt.close failed: {error}"))
                })?;
            Ok(())
        })
    }

    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        match adapter_state().await? {
            AdapterState::PoweredOn => {}
            AdapterState::PoweredOff => return Err(BluetoothError::PoweredOff),
            AdapterState::Unavailable | AdapterState::Unknown => {
                return Err(BluetoothError::NotAvailable);
            }
            AdapterState::Unauthorized => return Err(BluetoothError::PermissionDenied),
        }

        let callback_state_id = next_callback_state_id()?;
        let callback_state = Arc::new(GattCallbackState::new());
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();
        {
            let mut connect_slot = callback_state.connect.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("GATT connect slot mutex poisoned: {error}"))
            })?;
            *connect_slot = Some(connect_tx);
        }
        {
            let mut states = gatt_callbacks().lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT callback registry mutex poisoned: {error}"
                ))
            })?;
            states.insert(callback_state_id, Arc::clone(&callback_state));
        }

        let setup = with_android_context(|env, context| {
            init_dex(env, context)?;
            register_callback_natives(env)?;
            let helper_class = get_helper_class(env)?;
            let callback_class = load_class(env, "waterkit.bluetooth.BleGattBridgeCallback")?;
            let callback = env
                .new_object(callback_class, "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "new BleGattBridgeCallback failed: {error}"
                    ))
                })?;
            env.set_field(
                &callback,
                "waterkit_gatt_state",
                "J",
                JValue::Long(callback_state_id),
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "set BleGattBridgeCallback.waterkit_gatt_state failed: {error}"
                ))
            })?;

            let address = env.new_string(&device_id.0).map_err(|error| {
                BluetoothError::PlatformError(format!("new_string address failed: {error}"))
            })?;
            let gatt = env
                .call_static_method(
                    helper_class,
                    "connectGatt",
                    "(Landroid/content/Context;Ljava/lang/String;Landroid/bluetooth/BluetoothGattCallback;)Landroid/bluetooth/BluetoothGatt;",
                    &[
                        JValue::Object(context),
                        JValue::Object(&address),
                        JValue::Object(&callback),
                    ],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!("BluetoothHelper.connectGatt failed: {error}"))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!("connectGatt return decode failed: {error}"))
                })?;
            if gatt.is_null() {
                return Err(BluetoothError::DeviceNotFound(device_id.0.clone()));
            }

            let gatt = env.new_global_ref(gatt).map_err(|error| {
                BluetoothError::PlatformError(format!("new_global_ref gatt failed: {error}"))
            })?;
            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::PlatformError(format!("new_global_ref callback failed: {error}"))
            })?;
            Ok((gatt, callback))
        });

        let (gatt, callback) = match setup {
            Ok(values) => values,
            Err(error) => {
                if let Ok(mut states) = gatt_callbacks().lock() {
                    states.remove(&callback_state_id);
                }
                return Err(error);
            }
        };

        let connect_result = connect_rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("connect callback dropped: {error}"))
        })?;

        if let Err(error) = connect_result {
            let connection = Self {
                gatt,
                callback,
                callback_state_id,
            };
            if let Ok(mut states) = gatt_callbacks().lock() {
                states.remove(&callback_state_id);
            }
            let _ = connection.close_gatt();
            return Err(error);
        }

        Ok(Self {
            gatt,
            callback,
            callback_state_id,
        })
    }

    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let state = self.callback_state()?;
        let (tx, rx) = oneshot::channel::<Result<Vec<GattService>, BluetoothError>>();
        {
            let mut discover_slot = state.discover_services.lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT discover_services slot mutex poisoned: {error}"
                ))
            })?;
            if discover_slot.is_some() {
                return Err(BluetoothError::GattError(
                    "service discovery already pending".into(),
                ));
            }
            *discover_slot = Some(tx);
        }

        let started = with_android_context(|env, _context| {
            env.call_method(self.gatt.as_obj(), "discoverServices", "()Z", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGatt.discoverServices failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGatt.discoverServices return decode failed: {error}"
                    ))
                })
        })?;
        if !started {
            let mut discover_slot = state.discover_services.lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT discover_services slot mutex poisoned: {error}"
                ))
            })?;
            discover_slot.take();
            return Err(BluetoothError::GattError(
                "BluetoothGatt.discoverServices returned false".into(),
            ));
        }

        rx.await.map_err(|error| {
            BluetoothError::GattError(format!("discoverServices callback dropped: {error}"))
        })?
    }

    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        let state = self.callback_state()?;
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = oneshot::channel::<Result<Vec<u8>, BluetoothError>>();
        {
            let mut reads = state.reads.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("GATT read map mutex poisoned: {error}"))
            })?;
            if reads.insert(key.clone(), tx).is_some() {
                return Err(BluetoothError::GattError(format!(
                    "duplicate pending read for service={} characteristic={}",
                    key.0, key.1
                )));
            }
        }

        let started = with_android_context(|env, _context| {
            let characteristic_obj =
                jni_characteristic(env, self.gatt.as_obj(), &service.0, &characteristic.0)?;
            env.call_method(
                self.gatt.as_obj(),
                "readCharacteristic",
                "(Landroid/bluetooth/BluetoothGattCharacteristic;)Z",
                &[JValue::Object(&characteristic_obj)],
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.readCharacteristic failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.readCharacteristic return decode failed: {error}"
                ))
            })
        })?;

        if !started {
            let mut reads = state.reads.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("GATT read map mutex poisoned: {error}"))
            })?;
            reads.remove(&key);
            return Err(BluetoothError::GattError(
                "BluetoothGatt.readCharacteristic returned false".into(),
            ));
        }

        rx.await.map_err(|error| {
            BluetoothError::GattError(format!("readCharacteristic callback dropped: {error}"))
        })?
    }

    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        let state = self.callback_state()?;
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = oneshot::channel::<Result<(), BluetoothError>>();
        {
            let mut writes = state.writes.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("GATT write map mutex poisoned: {error}"))
            })?;
            if writes.insert(key.clone(), tx).is_some() {
                return Err(BluetoothError::GattError(format!(
                    "duplicate pending write for service={} characteristic={}",
                    key.0, key.1
                )));
            }
        }

        let started = with_android_context(|env, _context| {
            let characteristic_obj =
                jni_characteristic(env, self.gatt.as_obj(), &service.0, &characteristic.0)?;
            let payload = env.byte_array_from_slice(data).map_err(|error| {
                BluetoothError::PlatformError(format!("byte_array_from_slice failed: {error}"))
            })?;
            let set_value = env
                .call_method(
                    &characteristic_obj,
                    "setValue",
                    "([B)Z",
                    &[JValue::Object(&JObject::from(payload))],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattCharacteristic.setValue failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattCharacteristic.setValue return decode failed: {error}"
                    ))
                })?;
            if !set_value {
                return Ok(false);
            }
            env.call_method(
                self.gatt.as_obj(),
                "writeCharacteristic",
                "(Landroid/bluetooth/BluetoothGattCharacteristic;)Z",
                &[JValue::Object(&characteristic_obj)],
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.writeCharacteristic failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.writeCharacteristic return decode failed: {error}"
                ))
            })
        })?;

        if !started {
            let mut writes = state.writes.lock().map_err(|error| {
                BluetoothError::PlatformError(format!("GATT write map mutex poisoned: {error}"))
            })?;
            writes.remove(&key);
            return Err(BluetoothError::GattError(
                "BluetoothGatt.writeCharacteristic returned false".into(),
            ));
        }

        rx.await.map_err(|error| {
            BluetoothError::GattError(format!("writeCharacteristic callback dropped: {error}"))
        })?
    }

    #[allow(clippy::unused_async)]
    pub async fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let state = self.callback_state()?;
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = async_channel::unbounded();
        {
            let mut subscriptions = state.subscriptions.lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT subscription map mutex poisoned: {error}"
                ))
            })?;
            subscriptions.insert(key.clone(), tx);
        }

        let enabled = with_android_context(|env, _context| {
            let characteristic_obj =
                jni_characteristic(env, self.gatt.as_obj(), &service.0, &characteristic.0)?;
            let notification_set = env
                .call_method(
                    self.gatt.as_obj(),
                    "setCharacteristicNotification",
                    "(Landroid/bluetooth/BluetoothGattCharacteristic;Z)Z",
                    &[JValue::Object(&characteristic_obj), JValue::Bool(1)],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGatt.setCharacteristicNotification failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "setCharacteristicNotification return decode failed: {error}"
                    ))
                })?;
            if !notification_set {
                return Ok(false);
            }

            let descriptor_uuid =
                jni_uuid_from_string(env, "00002902-0000-1000-8000-00805f9b34fb")?;
            let descriptor = env
                .call_method(
                    &characteristic_obj,
                    "getDescriptor",
                    "(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattDescriptor;",
                    &[JValue::Object(&descriptor_uuid)],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattCharacteristic.getDescriptor failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattCharacteristic.getDescriptor return decode failed: {error}"
                    ))
                })?;
            if descriptor.is_null() {
                return Err(BluetoothError::GattError(
                    "CCCD descriptor not found for characteristic".into(),
                ));
            }

            let descriptor_class = env
                .find_class("android/bluetooth/BluetoothGattDescriptor")
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "find BluetoothGattDescriptor class failed: {error}"
                    ))
                })?;
            let enable_value = env
                .get_static_field(descriptor_class, "ENABLE_NOTIFICATION_VALUE", "[B")
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "read ENABLE_NOTIFICATION_VALUE failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "decode ENABLE_NOTIFICATION_VALUE failed: {error}"
                    ))
                })?;
            let set_descriptor = env
                .call_method(
                    &descriptor,
                    "setValue",
                    "([B)Z",
                    &[JValue::Object(&enable_value)],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattDescriptor.setValue failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGattDescriptor.setValue return decode failed: {error}"
                    ))
                })?;
            if !set_descriptor {
                return Ok(false);
            }

            env.call_method(
                self.gatt.as_obj(),
                "writeDescriptor",
                "(Landroid/bluetooth/BluetoothGattDescriptor;)Z",
                &[JValue::Object(&descriptor)],
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.writeDescriptor failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothGatt.writeDescriptor return decode failed: {error}"
                ))
            })
        })?;

        if !enabled {
            let mut subscriptions = state.subscriptions.lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "GATT subscription map mutex poisoned: {error}"
                ))
            })?;
            subscriptions.remove(&key);
            return Err(BluetoothError::GattError(
                "enable characteristic notifications failed".into(),
            ));
        }

        Ok(rx)
    }

    pub async fn disconnect(self) {
        let _ = self.close_gatt();
        if let Ok(mut states) = gatt_callbacks().lock() {
            states.remove(&self.callback_state_id);
        }
        let _ = &self.callback;
    }
}

impl Drop for BleConnectionInner {
    fn drop(&mut self) {
        let _ = with_android_context(|env, _context| {
            env.call_method(self.gatt.as_obj(), "disconnect", "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothGatt.disconnect failed: {error}"
                    ))
                })?;
            env.call_method(self.gatt.as_obj(), "close", "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!("BluetoothGatt.close failed: {error}"))
                })?;
            Ok(())
        });
        if let Ok(mut states) = gatt_callbacks().lock() {
            states.remove(&self.callback_state_id);
        }
        let _ = &self.callback;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_ClassicDiscoveryBridgeCallback_onDeviceFoundNative(
    mut env: JNIEnv,
    callback: JObject,
    device_address: JString,
    device_name: JObject,
    major_device_class: jni::sys::jint,
    is_paired: jni::sys::jboolean,
) {
    let callback_state_id =
        callback_state_id(&mut env, &callback, "waterkit_classic_discovery_state");
    let sender = {
        let callbacks = classic_discovery_callbacks()
            .lock()
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-bluetooth: classic discovery callback registry mutex poisoned: {error}"
                )
            });
        callbacks.get(&callback_state_id).cloned()
    };
    let Some(sender) = sender else {
        debug_assert!(
            false,
            "waterkit-bluetooth: missing classic discovery callback state for id {callback_state_id}"
        );
        return;
    };

    let device_address: String = env
        .get_string(&device_address)
        .unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode deviceAddress failed in classic discovery callback: {error}"
            )
        })
        .into();
    let device_name = if device_name.is_null() {
        None
    } else {
        let value: String = env
            .get_string(&JString::from(device_name))
            .unwrap_or_else(|error| {
                panic!(
                    "waterkit-bluetooth: decode deviceName failed in classic discovery callback: {error}"
                )
            })
            .into();
        if value.is_empty() { None } else { Some(value) }
    };

    if let Err(error) = sender.try_send(ClassicDevice {
        device: BluetoothDevice {
            id: DeviceId(device_address),
            name: device_name,
            rssi: None,
            is_connected: false,
        },
        device_class: major_device_class as u32,
        is_paired: is_paired != 0,
    }) {
        debug_assert!(
            false,
            "waterkit-bluetooth: dropping classic discovery result because receiver is closed: {error}"
        );
    }
}

pub struct ClassicBluetoothInner {
    discovery_session: Mutex<Option<ClassicDiscoverySession>>,
}

impl std::fmt::Debug for ClassicBluetoothInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassicBluetoothInner").finish()
    }
}

impl ClassicBluetoothInner {
    pub async fn new() -> Result<Self, BluetoothError> {
        match adapter_state().await? {
            AdapterState::PoweredOn => Ok(Self {
                discovery_session: Mutex::new(None),
            }),
            AdapterState::PoweredOff => Err(BluetoothError::PoweredOff),
            AdapterState::Unavailable | AdapterState::Unknown => Err(BluetoothError::NotAvailable),
            AdapterState::Unauthorized => Err(BluetoothError::PermissionDenied),
        }
    }

    #[allow(clippy::unused_async)]
    pub async fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        {
            let session = self.discovery_session.lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "classic discovery session mutex poisoned: {error}"
                ))
            })?;
            if session.is_some() {
                return Err(BluetoothError::PlatformError(
                    "classic discovery already active".into(),
                ));
            }
        }

        let callback_state_id = next_callback_state_id()?;
        let (tx, rx) = async_channel::unbounded();
        let session = with_android_context(|env, context| {
            init_dex(env, context)?;
            register_callback_natives(env)?;
            let helper_class = get_helper_class(env)?;
            let callback_class =
                load_class(env, "waterkit.bluetooth.ClassicDiscoveryBridgeCallback")?;
            let callback = env
                .new_object(callback_class, "()V", &[])
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "new ClassicDiscoveryBridgeCallback failed: {error}"
                    ))
                })?;
            env.set_field(
                &callback,
                "waterkit_classic_discovery_state",
                "J",
                JValue::Long(callback_state_id),
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "set waterkit_classic_discovery_state failed: {error}"
                ))
            })?;

            let started = env
                .call_static_method(
                    helper_class,
                    "startClassicDiscovery",
                    "(Landroid/content/Context;Lwaterkit/bluetooth/ClassicDiscoveryCallback;)Z",
                    &[JValue::Object(context), JValue::Object(&callback)],
                )
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "BluetoothHelper.startClassicDiscovery failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::PlatformError(format!(
                        "startClassicDiscovery return decode failed: {error}"
                    ))
                })?;
            if !started {
                return Err(BluetoothError::GattError(
                    "BluetoothAdapter.startDiscovery returned false".into(),
                ));
            }

            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::PlatformError(format!("new_global_ref callback failed: {error}"))
            })?;
            Ok(ClassicDiscoverySession {
                callback,
                callback_state_id,
            })
        })?;

        {
            let mut callbacks = classic_discovery_callbacks().lock().map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "classic discovery callback registry mutex poisoned: {error}"
                ))
            })?;
            callbacks.insert(callback_state_id, tx);
        }

        let mut stored_session = self.discovery_session.lock().map_err(|error| {
            BluetoothError::PlatformError(format!(
                "classic discovery session mutex poisoned: {error}"
            ))
        })?;
        *stored_session = Some(session);
        Ok(rx)
    }

    fn stop_discovery_impl(&self) -> Result<(), BluetoothError> {
        let session = self
            .discovery_session
            .lock()
            .ok()
            .and_then(|mut session| session.take());
        let Some(session) = session else {
            return Ok(());
        };

        if let Ok(mut callbacks) = classic_discovery_callbacks().lock() {
            callbacks.remove(&session.callback_state_id);
        }

        let result = with_android_context(|env, context| {
            init_dex(env, context)?;
            let helper_class = get_helper_class(env)?;
            env.call_static_method(
                helper_class,
                "stopClassicDiscovery",
                "(Landroid/content/Context;)V",
                &[JValue::Object(context)],
            )
            .map_err(|error| {
                BluetoothError::PlatformError(format!(
                    "BluetoothHelper.stopClassicDiscovery failed: {error}"
                ))
            })?;
            Ok(())
        });

        let _ = &session.callback;
        result
    }

    #[allow(clippy::unused_async)]
    pub async fn stop_discovery(&self) {
        let _ = self.stop_discovery_impl();
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        future::ready(with_android_context(get_paired_devices_with_context)).await
    }

    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        let device_id = device_id.0.clone();
        let service_uuid = uuid.0.clone();
        let (command_tx, command_rx) = async_channel::unbounded();
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();

        let worker = std::thread::Builder::new()
            .name("waterkit-spp-android".to_owned())
            .spawn(move || {
                let socket = with_android_context(|env, context| {
                    init_dex(env, context)?;
                    let helper_class = get_helper_class(env)?;
                    let device_id = env.new_string(device_id).map_err(|error| {
                        BluetoothError::PlatformError(format!("new_string device_id failed: {error}"))
                    })?;
                    let service_uuid = env.new_string(service_uuid).map_err(|error| {
                        BluetoothError::PlatformError(format!(
                            "new_string service_uuid failed: {error}"
                        ))
                    })?;
                    let socket = env
                        .call_static_method(
                            helper_class,
                            "connectSpp",
                            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Landroid/bluetooth/BluetoothSocket;",
                            &[
                                JValue::Object(context),
                                JValue::Object(&device_id),
                                JValue::Object(&service_uuid),
                            ],
                        )
                        .map_err(|error| {
                            BluetoothError::PlatformError(format!(
                                "BluetoothHelper.connectSpp failed: {error}"
                            ))
                        })?
                        .l()
                        .map_err(|error| {
                            BluetoothError::PlatformError(format!(
                                "connectSpp return decode failed: {error}"
                            ))
                        })?;
                    if socket.is_null() {
                        return Err(BluetoothError::ConnectionFailed(
                            "connectSpp returned null socket".into(),
                        ));
                    }
                    env.new_global_ref(socket).map_err(|error| {
                        BluetoothError::PlatformError(format!(
                            "new_global_ref BluetoothSocket failed: {error}"
                        ))
                    })
                });

                let socket = match socket {
                    Ok(socket) => {
                        let _ = connect_tx.send(Ok(()));
                        socket
                    }
                    Err(error) => {
                        let _ = connect_tx.send(Err(error));
                        return;
                    }
                };

                while let Ok(command) = command_rx.recv_blocking() {
                    match command {
                        SppCommand::Read { max_bytes, tx } => {
                            let result = with_android_context(|env, context| {
                                init_dex(env, context)?;
                                let helper_class = get_helper_class(env)?;
                                let max_bytes = i32::try_from(max_bytes).map_err(|_| {
                                    BluetoothError::PlatformError(format!(
                                        "SPP read size exceeds i32: {max_bytes}"
                                    ))
                                })?;
                                let bytes = env
                                    .call_static_method(
                                        helper_class,
                                        "readSpp",
                                        "(Landroid/bluetooth/BluetoothSocket;I)[B",
                                        &[
                                            JValue::Object(socket.as_obj()),
                                            JValue::Int(max_bytes),
                                        ],
                                    )
                                    .map_err(|error| {
                                        BluetoothError::PlatformError(format!(
                                            "BluetoothHelper.readSpp failed: {error}"
                                        ))
                                    })?
                                    .l()
                                    .map_err(|error| {
                                        BluetoothError::PlatformError(format!(
                                            "readSpp return decode failed: {error}"
                                        ))
                                    })?;
                                if bytes.is_null() {
                                    return Err(BluetoothError::ConnectionFailed(
                                        "SPP stream closed".into(),
                                    ));
                                }
                                env.convert_byte_array(JByteArray::from(bytes)).map_err(|error| {
                                    BluetoothError::PlatformError(format!(
                                        "decode readSpp byte array failed: {error}"
                                    ))
                                })
                            });
                            let _ = tx.send(result);
                        }
                        SppCommand::Write { data, tx } => {
                            let result = with_android_context(|env, context| {
                                init_dex(env, context)?;
                                let helper_class = get_helper_class(env)?;
                                let payload = env.byte_array_from_slice(&data).map_err(|error| {
                                    BluetoothError::PlatformError(format!(
                                        "byte_array_from_slice failed in writeSpp: {error}"
                                    ))
                                })?;
                                let written = env
                                    .call_static_method(
                                        helper_class,
                                        "writeSpp",
                                        "(Landroid/bluetooth/BluetoothSocket;[B)I",
                                        &[
                                            JValue::Object(socket.as_obj()),
                                            JValue::Object(&JObject::from(payload)),
                                        ],
                                    )
                                    .map_err(|error| {
                                        BluetoothError::PlatformError(format!(
                                            "BluetoothHelper.writeSpp failed: {error}"
                                        ))
                                    })?
                                    .i()
                                    .map_err(|error| {
                                        BluetoothError::PlatformError(format!(
                                            "writeSpp return decode failed: {error}"
                                        ))
                                    })?;
                                usize::try_from(written).map_err(|_| {
                                    BluetoothError::PlatformError(format!(
                                        "writeSpp returned negative byte count: {written}"
                                    ))
                                })
                            });
                            let _ = tx.send(result);
                        }
                        SppCommand::Close { tx } => {
                            let _ = with_android_context(|env, context| {
                                init_dex(env, context)?;
                                let helper_class = get_helper_class(env)?;
                                env.call_static_method(
                                    helper_class,
                                    "closeSpp",
                                    "(Landroid/bluetooth/BluetoothSocket;)V",
                                    &[JValue::Object(socket.as_obj())],
                                )
                                .map_err(|error| {
                                    BluetoothError::PlatformError(format!(
                                        "BluetoothHelper.closeSpp failed: {error}"
                                    ))
                                })?;
                                Ok(())
                            });
                            let _ = tx.send(());
                            break;
                        }
                    }
                }

                let _ = with_android_context(|env, context| {
                    init_dex(env, context)?;
                    let helper_class = get_helper_class(env)?;
                    env.call_static_method(
                        helper_class,
                        "closeSpp",
                        "(Landroid/bluetooth/BluetoothSocket;)V",
                        &[JValue::Object(socket.as_obj())],
                    )
                    .map_err(|error| {
                        BluetoothError::PlatformError(format!(
                            "BluetoothHelper.closeSpp failed during worker shutdown: {error}"
                        ))
                    })?;
                    Ok(())
                });
            })
            .map_err(|error| {
                BluetoothError::PlatformError(format!("spawn SPP worker failed: {error}"))
            })?;

        match connect_rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP connect callback dropped: {error}"))
        })? {
            Ok(()) => Ok(SppStreamInner {
                command_tx,
                worker: Mutex::new(Some(worker)),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }
}

impl Drop for ClassicBluetoothInner {
    fn drop(&mut self) {
        let _ = self.stop_discovery_impl();
    }
}

pub struct SppStreamInner {
    command_tx: async_channel::Sender<SppCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for SppStreamInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SppStreamInner").finish()
    }
}

impl SppStreamInner {
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(SppCommand::Read {
                max_bytes: buf.len(),
                tx,
            })
            .await
            .map_err(|error| {
                BluetoothError::ConnectionFailed(format!("SPP read command send failed: {error}"))
            })?;
        let data = rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP read response dropped: {error}"))
        })??;
        let read = data.len().min(buf.len());
        buf[..read].copy_from_slice(&data[..read]);
        Ok(read)
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(SppCommand::Write {
                data: data.to_vec(),
                tx,
            })
            .await
            .map_err(|error| {
                BluetoothError::ConnectionFailed(format!("SPP write command send failed: {error}"))
            })?;
        rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("SPP write response dropped: {error}"))
        })?
    }

    pub async fn close(self) {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(SppCommand::Close { tx }).await;
        let _ = rx.await;
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for SppStreamInner {
    fn drop(&mut self) {
        let (tx, _rx) = oneshot::channel();
        let _ = self.command_tx.try_send(SppCommand::Close { tx });
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// Android-specific Bluetooth functions requiring JNI context.
pub mod jni_api {
    use super::*;

    /// Get adapter state with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn get_adapter_state(
        env: &mut JNIEnv,
        context: &JObject,
    ) -> Result<AdapterState, BluetoothError> {
        init_dex(env, context)?;
        let helper_class = get_helper_class(env)?;
        let state = env
            .call_static_method(
                &helper_class,
                "getAdapterState",
                "(Landroid/content/Context;)I",
                &[JValue::Object(context)],
            )
            .map_err(|e| BluetoothError::PlatformError(format!("getAdapterState: {e}")))?
            .i()
            .map_err(|e| BluetoothError::PlatformError(format!("return: {e}")))?;
        match state {
            0 => Ok(AdapterState::PoweredOff),
            1 => Ok(AdapterState::PoweredOn),
            2 => Ok(AdapterState::Unavailable),
            3 => Ok(AdapterState::Unauthorized),
            _ => Ok(AdapterState::Unknown),
        }
    }
}
