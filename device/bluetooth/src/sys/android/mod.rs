use crate::{
    AdapterState, BluetoothDevice, BluetoothError, CharacteristicProperties, ClassicDevice,
    DeviceId, GattCharacteristic, GattService, ScanFilter, ScanResult, Uuid,
};
use futures::channel::oneshot;
use futures::future;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JByteArray, JClass, JObject, JObjectArray, JString, JValue};
use jni::strings::JNIStr;
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
const HELPER_CLASS_NAME: &str = "waterkit.bluetooth.BluetoothHelper";
const BOND_BONDED: i32 = 12;
type GattResultSender<T> = oneshot::Sender<Result<T, BluetoothError>>;
type CharacteristicKey = (String, String);
type SubscriptionMap = BTreeMap<CharacteristicKey, async_channel::Sender<Vec<u8>>>;

impl From<jni::errors::Error> for BluetoothError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Platform(format!("Android JNI operation failed: {error}"))
    }
}

#[derive(Debug, Clone)]
struct ScanCallbackState {
    sender: async_channel::Sender<ScanResult>,
    name_prefix: Option<String>,
}

#[derive(Debug)]
struct BleScanSession {
    scanner: Global<JObject<'static>>,
    callback: Global<JObject<'static>>,
    _callback_state: Arc<ScanCallbackState>,
}

#[derive(Debug)]
struct ClassicDiscoverySession {
    callback: Global<JObject<'static>>,
    _callback_state: Arc<async_channel::Sender<ClassicDevice>>,
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
    connect: Mutex<Option<GattResultSender<()>>>,
    discover_services: Mutex<Option<GattResultSender<Vec<GattService>>>>,
    reads: Mutex<BTreeMap<CharacteristicKey, GattResultSender<Vec<u8>>>>,
    writes: Mutex<BTreeMap<CharacteristicKey, GattResultSender<()>>>,
    subscriptions: Mutex<SubscriptionMap>,
}

impl GattCallbackState {
    const fn new() -> Self {
        Self {
            connect: Mutex::new(None),
            discover_services: Mutex::new(None),
            reads: Mutex::new(BTreeMap::new()),
            writes: Mutex::new(BTreeMap::new()),
            subscriptions: Mutex::new(BTreeMap::new()),
        }
    }
}

fn callback_state_handle<T>(state: &Arc<T>) -> Result<i64, BluetoothError> {
    i64::try_from(Arc::as_ptr(state).expose_provenance()).map_err(|_| {
        BluetoothError::Platform("Android callback state pointer exceeds Java long".into())
    })
}

fn callback_state<T>(env: &mut Env<'_>, callback: &JObject, field: &JNIStr) -> Arc<T> {
    let value = env
        .get_field(callback, field, jni_sig!("J"))
        .unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: read {field} failed in callback: {error}")
        });
    let state_handle = value.j().unwrap_or_else(|error| {
        panic!("waterkit-bluetooth: decode {field} failed in callback: {error}")
    });
    let state_address = usize::try_from(state_handle).unwrap_or_else(|_| {
        panic!("waterkit-bluetooth: invalid callback state pointer in {field}: {state_handle}")
    });
    assert_ne!(
        state_address, 0,
        "waterkit-bluetooth: null callback state pointer in {field}"
    );
    let state = std::ptr::with_exposed_provenance::<T>(state_address);
    // SAFETY: every callback bridge serializes native invocation with
    // `releaseNativeState`; the owning Rust session keeps the original Arc
    // alive until that synchronized release returns.
    unsafe {
        Arc::increment_strong_count(state);
        Arc::from_raw(state)
    }
}

fn with_callback_env<'caller>(
    env: &mut EnvUnowned<'caller>,
    callback: impl FnOnce(&mut Env<'caller>) -> jni::errors::Result<()>,
) {
    env.with_env(callback).resolve::<ThrowRuntimeExAndDefault>();
}

fn release_callback_state(
    env: &mut Env<'_>,
    callback: &Global<JObject<'static>>,
) -> Result<(), BluetoothError> {
    env.call_method(
        callback.as_obj(),
        jni_str!("releaseNativeState"),
        jni_sig!("()V"),
        &[],
    )
    .map_err(|error| {
        BluetoothError::Platform(format!("release callback native state failed: {error}"))
    })?;
    Ok(())
}

fn with_android_context<T, F>(f: F) -> Result<T, BluetoothError>
where
    F: for<'local> FnOnce(&mut Env<'local>, &JObject<'static>) -> Result<T, BluetoothError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-bluetooth: ndk_context returned null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-bluetooth: ndk_context returned null Android Context"
    );

    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    let context = vm.attach_current_thread(|env| {
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context) }?;
        env.new_global_ref(&*context).map_err(BluetoothError::from)
    })?;
    vm.attach_current_thread(|env| f(env, context.as_obj()))
}

fn init_dex(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JObject<'static>>, BluetoothError> {
    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|error| BluetoothError::Platform(format!("create DEX byte array: {error}")))?;
    let dex_bytes = JObject::from(dex_bytes);
    let byte_buffer_class = env
        .find_class(jni_str!("java/nio/ByteBuffer"))
        .map_err(|error| BluetoothError::Platform(format!("find ByteBuffer: {error}")))?;
    let byte_buffer = env
        .call_static_method(
            byte_buffer_class,
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .and_then(jni::JValueOwned::l)
        .map_err(|error| BluetoothError::Platform(format!("ByteBuffer.wrap DEX: {error}")))?;
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .and_then(jni::JValueOwned::l)
        .map_err(|e| BluetoothError::Platform(format!("getClassLoader: {e}")))?;
    let dex_class = env
        .find_class(jni_str!("dalvik/system/InMemoryDexClassLoader"))
        .map_err(|e| BluetoothError::Platform(format!("find_class: {e}")))?;
    let loader = env
        .new_object(
            dex_class,
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&byte_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|e| BluetoothError::Platform(format!("new_object: {e}")))?;
    env.new_global_ref(loader)
        .map_err(|e| BluetoothError::Platform(format!("global_ref: {e}")))
}

fn load_class<'local>(
    env: &mut Env<'local>,
    loader: &Global<JObject<'static>>,
    class_name: &str,
) -> Result<JClass<'local>, BluetoothError> {
    let class_name = env
        .new_string(class_name)
        .map_err(|e| BluetoothError::Platform(format!("new_string class_name: {e}")))?;
    let class = env
        .call_method(
            loader.as_obj(),
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&class_name)],
        )
        .and_then(jni::JValueOwned::l)
        .map_err(|e| BluetoothError::Platform(format!("loadClass: {e}")))?;
    env.cast_local::<JClass>(class)
        .map_err(|error| BluetoothError::Platform(format!("cast loaded class: {error}")))
}

fn register_callback_natives(
    env: &mut Env<'_>,
    loader: &Global<JObject<'static>>,
) -> Result<(), BluetoothError> {
    let scan_callback_class = load_class(env, loader, "waterkit.bluetooth.BleScanBridgeCallback")?;
    let scan_natives = [unsafe {
        jni::NativeMethod::from_raw_parts(
            jni_str!("onScanResultNative"),
            jni_str!("(Ljava/lang/String;Ljava/lang/String;I[Ljava/lang/String;)V"),
            Java_waterkit_bluetooth_BleScanBridgeCallback_onScanResultNative as *mut _,
        )
    }];
    // SAFETY: every descriptor above names the exact Kotlin instance method
    // signature implemented by its corresponding `extern "system"` function.
    unsafe { env.register_native_methods(scan_callback_class, &scan_natives) }.map_err(|e| {
        BluetoothError::Platform(format!(
            "register_native_methods BleScanBridgeCallback failed: {e}"
        ))
    })?;

    let gatt_callback_class = load_class(env, loader, "waterkit.bluetooth.BleGattBridgeCallback")?;
    let gatt_natives = [
        unsafe {
            jni::NativeMethod::from_raw_parts(
                jni_str!("onConnectionStateNative"),
                jni_str!("(Ljava/lang/String;ZI)V"),
                Java_waterkit_bluetooth_BleGattBridgeCallback_onConnectionStateNative as *mut _,
            )
        },
        unsafe {
            jni::NativeMethod::from_raw_parts(
                jni_str!("onServicesDiscoveredNative"),
                jni_str!("(Ljava/lang/String;Ljava/lang/String;I)V"),
                Java_waterkit_bluetooth_BleGattBridgeCallback_onServicesDiscoveredNative as *mut _,
            )
        },
        unsafe {
            jni::NativeMethod::from_raw_parts(
                jni_str!("onCharacteristicReadNative"),
                jni_str!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[BI)V"),
                Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicReadNative as *mut _,
            )
        },
        unsafe {
            jni::NativeMethod::from_raw_parts(
                jni_str!("onCharacteristicWriteNative"),
                jni_str!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V"),
                Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicWriteNative as *mut _,
            )
        },
        unsafe {
            jni::NativeMethod::from_raw_parts(
                jni_str!("onCharacteristicChangedNative"),
                jni_str!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V"),
                Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicChangedNative
                    as *mut _,
            )
        },
    ];
    // SAFETY: each GATT descriptor matches the parameter and return types of
    // the paired `extern "system"` callback.
    unsafe { env.register_native_methods(gatt_callback_class, &gatt_natives) }.map_err(|e| {
        BluetoothError::Platform(format!(
            "register_native_methods BleGattBridgeCallback failed: {e}"
        ))
    })?;

    let classic_callback_class = load_class(
        env,
        loader,
        "waterkit.bluetooth.ClassicDiscoveryBridgeCallback",
    )?;
    let classic_natives = [unsafe {
        jni::NativeMethod::from_raw_parts(
            jni_str!("onDeviceFoundNative"),
            jni_str!("(Ljava/lang/String;Ljava/lang/String;IZ)V"),
            Java_waterkit_bluetooth_ClassicDiscoveryBridgeCallback_onDeviceFoundNative as *mut _,
        )
    }];
    // SAFETY: the descriptor matches the Classic discovery callback ABI.
    unsafe { env.register_native_methods(classic_callback_class, &classic_natives) }.map_err(
        |e| {
            BluetoothError::Platform(format!(
                "register_native_methods ClassicDiscoveryBridgeCallback failed: {e}"
            ))
        },
    )?;

    Ok(())
}

pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    future::ready(with_android_context(jni_api::get_adapter_state)).await
}

fn get_helper_class<'local>(
    env: &mut Env<'local>,
    loader: &Global<JObject<'static>>,
) -> Result<jni::objects::JClass<'local>, BluetoothError> {
    load_class(env, loader, HELPER_CLASS_NAME)
}

#[allow(
    clippy::too_many_lines,
    reason = "This JNI bridge decodes one Android BluetoothDevice payload end-to-end; splitting it would obscure the field extraction order."
)]
fn get_paired_devices_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Vec<ClassicDevice>, BluetoothError> {
    let loader = init_dex(env, context)?;
    let helper_class = get_helper_class(env, &loader)?;
    let paired_obj = env
        .call_static_method(
            &helper_class,
            jni_str!("getPairedDevices"),
            jni_sig!("(Landroid/content/Context;)[Landroid/bluetooth/BluetoothDevice;"),
            &[JValue::Object(context)],
        )
        .map_err(|e| BluetoothError::Platform(format!("getPairedDevices: {e}")))?
        .l()
        .map_err(|e| BluetoothError::Platform(format!("pairedDevices return: {e}")))?;

    if paired_obj.is_null() {
        return Err(BluetoothError::Platform(
            "BluetoothHelper.getPairedDevices returned null".into(),
        ));
    }

    let paired = JObjectArray::<JObject>::cast_local(env, paired_obj)
        .map_err(|error| BluetoothError::Platform(format!("cast paired device array: {error}")))?;
    let count = paired
        .len(env)
        .map_err(|e| BluetoothError::Platform(format!("get_array_length: {e}")))?;
    let mut devices = Vec::with_capacity(count);

    for index in 0..count {
        let device_obj = paired
            .get_element(env, index)
            .map_err(|e| BluetoothError::Platform(format!("get_object_array_element: {e}")))?;
        if device_obj.is_null() {
            continue;
        }

        let address_obj = env
            .call_method(
                &device_obj,
                jni_str!("getAddress"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .map_err(|e| BluetoothError::Platform(format!("BluetoothDevice.getAddress: {e}")))?
            .l()
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothDevice.getAddress return decode: {e}"))
            })?;
        if address_obj.is_null() {
            return Err(BluetoothError::Platform(
                "BluetoothDevice.getAddress returned null".into(),
            ));
        }
        let address = env
            .as_cast::<JString>(&address_obj)
            .and_then(|address| address.try_to_string(env))
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothDevice.getAddress get_string: {e}"))
            })?;

        let name_obj = env
            .call_method(
                &device_obj,
                jni_str!("getName"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .map_err(|e| BluetoothError::Platform(format!("BluetoothDevice.getName: {e}")))?
            .l()
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothDevice.getName return decode: {e}"))
            })?;
        let name = if name_obj.is_null() {
            None
        } else {
            let value = env
                .as_cast::<JString>(&name_obj)
                .and_then(|name| name.try_to_string(env))
                .map_err(|e| {
                    BluetoothError::Platform(format!("BluetoothDevice.getName get_string: {e}"))
                })?;
            if value.is_empty() { None } else { Some(value) }
        };

        let class_obj = env
            .call_method(
                &device_obj,
                jni_str!("getBluetoothClass"),
                jni_sig!("()Landroid/bluetooth/BluetoothClass;"),
                &[],
            )
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothDevice.getBluetoothClass: {e}"))
            })?
            .l()
            .map_err(|e| {
                BluetoothError::Platform(format!(
                    "BluetoothDevice.getBluetoothClass return decode: {e}"
                ))
            })?;
        let device_class = if class_obj.is_null() {
            0
        } else {
            env.call_method(
                &class_obj,
                jni_str!("getMajorDeviceClass"),
                jni_sig!("()I"),
                &[],
            )
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothClass.getMajorDeviceClass: {e}"))
            })?
            .i()
            .map_err(|e| {
                BluetoothError::Platform(format!(
                    "BluetoothClass.getMajorDeviceClass return decode: {e}"
                ))
            })?
            .cast_unsigned()
        };

        let bond_state = env
            .call_method(&device_obj, jni_str!("getBondState"), jni_sig!("()I"), &[])
            .map_err(|e| BluetoothError::Platform(format!("BluetoothDevice.getBondState: {e}")))?
            .i()
            .map_err(|e| {
                BluetoothError::Platform(format!("BluetoothDevice.getBondState return decode: {e}"))
            })?;

        devices.push(ClassicDevice {
            device: BluetoothDevice {
                id: DeviceId::new(address),
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

    #[allow(
        clippy::too_many_lines,
        reason = "Starting an Android BLE scan requires a single JNI transaction that creates the callback, filter array, and scanner together."
    )]
    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        {
            let session = self.session.lock().map_err(|error| {
                BluetoothError::Platform(format!("BLE scan session mutex poisoned: {error}"))
            })?;
            if session.is_some() {
                return Err(BluetoothError::Platform(
                    "BLE scan already active on this scanner".into(),
                ));
            }
        }

        let (tx, rx) = async_channel::unbounded();
        let scan_state = Arc::new(ScanCallbackState {
            sender: tx,
            name_prefix: filter.name_prefix.clone(),
        });
        let callback_state_handle = callback_state_handle(&scan_state)?;
        let service_uuids: Vec<String> = filter
            .service_uuids
            .iter()
            .map(|uuid| uuid.as_str().to_string())
            .collect();

        let session = with_android_context(|env, context| {
            let loader = init_dex(env, context)?;
            register_callback_natives(env, &loader)?;
            let helper_class = get_helper_class(env, &loader)?;
            let callback_class =
                load_class(env, &loader, "waterkit.bluetooth.BleScanBridgeCallback")?;
            let callback = env
                .new_object(callback_class, jni_sig!("()V"), &[])
                .map_err(|error| {
                    BluetoothError::Platform(format!("new BleScanBridgeCallback failed: {error}"))
                })?;
            env.set_field(
                &callback,
                jni_str!("waterkit_scan_state"),
                jni_sig!("J"),
                JValue::Long(callback_state_handle),
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "set BleScanBridgeCallback.waterkit_scan_state failed: {error}"
                ))
            })?;
            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::Platform(format!("new_global_ref callback failed: {error}"))
            })?;

            let service_uuid_array = if service_uuids.is_empty() {
                JObject::null()
            } else {
                let array = JObjectArray::<JString>::new(env, service_uuids.len(), JString::null())
                    .map_err(|error| {
                        BluetoothError::Platform(format!(
                            "new service UUID object array failed: {error}"
                        ))
                    })?;

                for (index, uuid) in service_uuids.iter().enumerate() {
                    let value = env.new_string(uuid).map_err(|error| {
                        BluetoothError::Platform(format!("new_string UUID failed: {error}"))
                    })?;
                    array.set_element(env, index, &value).map_err(|error| {
                        BluetoothError::Platform(format!(
                            "set service UUID array element failed: {error}"
                        ))
                    })?;
                }
                JObject::from(array)
            };

            let scanner = env
                .call_static_method(
                    &helper_class,
                    jni_str!("startBleScan"),
                    jni_sig!(
                        "(Landroid/content/Context;[Ljava/lang/String;Lwaterkit/bluetooth/BleScanCallback;)Landroid/bluetooth/le/BluetoothLeScanner;"
                    ),
                    &[
                        JValue::Object(context),
                        JValue::Object(&service_uuid_array),
                        JValue::Object(callback.as_obj()),
                    ],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothHelper.startBleScan failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothHelper.startBleScan return decode failed: {error}"
                    ))
                })?;

            if scanner.is_null() {
                release_callback_state(env, &callback)?;
                return Err(BluetoothError::NotAvailable);
            }

            let scanner = match env.new_global_ref(&scanner) {
                Ok(scanner) => scanner,
                Err(error) => {
                    let _ = env.call_static_method(
                        &helper_class,
                        jni_str!("stopBleScan"),
                        jni_sig!(
                            "(Landroid/bluetooth/le/BluetoothLeScanner;Landroid/bluetooth/le/ScanCallback;)V"
                        ),
                        &[
                            JValue::Object(&scanner),
                            JValue::Object(callback.as_obj()),
                        ],
                    );
                    release_callback_state(env, &callback)?;
                    return Err(BluetoothError::Platform(format!(
                        "new_global_ref scanner failed: {error}"
                    )));
                }
            };

            Ok(BleScanSession {
                scanner,
                callback,
                _callback_state: Arc::clone(&scan_state),
            })
        })?;

        *self.session.lock().map_err(|error| {
            BluetoothError::Platform(format!("BLE scan session mutex poisoned: {error}"))
        })? = Some(session);
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

        with_android_context(|env, context| {
            release_callback_state(env, &session.callback)?;
            let loader = init_dex(env, context)?;
            let helper_class = get_helper_class(env, &loader)?;
            env.call_static_method(
                &helper_class,
                jni_str!("stopBleScan"),
                jni_sig!(
                    "(Landroid/bluetooth/le/BluetoothLeScanner;Landroid/bluetooth/le/ScanCallback;)V"
                ),
                &[
                    JValue::Object(session.scanner.as_obj()),
                    JValue::Object(session.callback.as_obj()),
                ],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!("BluetoothHelper.stopBleScan failed: {error}"))
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
#[allow(
    clippy::too_many_lines,
    reason = "JNI scan callbacks decode the full Android payload at the native boundary; keeping the decode in one callback preserves ownership and null-check order."
)]
pub extern "system" fn Java_waterkit_bluetooth_BleScanBridgeCallback_onScanResultNative<'caller>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    device_address: JString<'caller>,
    device_name: JObject<'caller>,
    rssi: jni::sys::jint,
    service_uuids: JObjectArray<'caller>,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<ScanCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_scan_state"));

        let device_address = device_address.try_to_string(env).unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode deviceAddress failed in scan callback: {error}")
        });

        let device_name = if device_name.is_null() {
            None
        } else {
            let value = env
                .as_cast::<JString>(&device_name)
                .and_then(|value| value.try_to_string(env))
                .unwrap_or_else(|error| {
                    panic!("waterkit-bluetooth: decode deviceName failed in scan callback: {error}")
                });
            if value.is_empty() { None } else { Some(value) }
        };

        if let Some(prefix) = state.name_prefix.as_ref()
            && !device_name
                .as_deref()
                .is_some_and(|name| name.starts_with(prefix))
        {
            return Ok(());
        }

        let service_uuids_len = service_uuids.len(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: get service UUID array length failed in scan callback: {error}"
            )
        });
        let mut parsed_uuids = Vec::with_capacity(service_uuids_len);
        for index in 0..service_uuids_len {
            let value = service_uuids
                .get_element(env, index)
                .unwrap_or_else(|error| {
                    panic!(
                        "waterkit-bluetooth: get service UUID array element failed at {index}: {error}"
                    )
                });
            if value.is_null() {
                continue;
            }
            let value = env
                .as_cast::<JString>(&value)
                .and_then(|value| value.try_to_string(env))
                .unwrap_or_else(|error| {
                    panic!("waterkit-bluetooth: decode service UUID failed at {index}: {error}")
                });
            if !value.is_empty() {
                parsed_uuids.push(Uuid::new(value));
            }
        }

        if let Err(error) = state.sender.try_send(ScanResult {
            device: BluetoothDevice {
                id: DeviceId::new(device_address),
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
        Ok(())
    });
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
                uuid: Uuid::new(parts[0]),
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
            uuid: Uuid::new(service_uuid),
            is_primary,
            characteristics,
        });
    }
    Ok(services)
}

fn jni_uuid_from_string<'local>(
    env: &mut Env<'local>,
    uuid: &str,
) -> Result<JObject<'local>, BluetoothError> {
    let uuid_class = env
        .find_class(jni_str!("java/util/UUID"))
        .map_err(|error| {
            BluetoothError::Platform(format!("find java/util/UUID failed: {error}"))
        })?;
    let uuid = env
        .new_string(uuid)
        .map_err(|error| BluetoothError::Platform(format!("new_string UUID failed: {error}")))?;
    env.call_static_method(
        uuid_class,
        jni_str!("fromString"),
        jni_sig!("(Ljava/lang/String;)Ljava/util/UUID;"),
        &[JValue::Object(&uuid)],
    )
    .map_err(|error| BluetoothError::Platform(format!("UUID.fromString failed: {error}")))?
    .l()
    .map_err(|error| BluetoothError::Platform(format!("UUID.fromString decode failed: {error}")))
}

fn jni_characteristic<'local>(
    env: &mut Env<'local>,
    gatt: &JObject<'local>,
    service_uuid: &str,
    characteristic_uuid: &str,
) -> Result<JObject<'local>, BluetoothError> {
    let service_uuid_obj = jni_uuid_from_string(env, service_uuid)?;
    let service = env
        .call_method(
            gatt,
            jni_str!("getService"),
            jni_sig!("(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattService;"),
            &[JValue::Object(&service_uuid_obj)],
        )
        .map_err(|error| {
            BluetoothError::Platform(format!("BluetoothGatt.getService failed: {error}"))
        })?
        .l()
        .map_err(|error| {
            BluetoothError::Platform(format!("BluetoothGatt.getService decode failed: {error}"))
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
            jni_str!("getCharacteristic"),
            jni_sig!("(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattCharacteristic;"),
            &[JValue::Object(&characteristic_uuid_obj)],
        )
        .map_err(|error| {
            BluetoothError::Platform(format!(
                "BluetoothGattService.getCharacteristic failed: {error}"
            ))
        })?
        .l()
        .map_err(|error| {
            BluetoothError::Platform(format!(
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
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onConnectionStateNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    _device_address: JString<'caller>,
    connected: jni::sys::jboolean,
    status: jni::sys::jint,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<GattCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_gatt_state"));

        let pending = state
            .connect
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: GATT connect slot mutex poisoned: {error}")
            })
            .take();
        if let Some(tx) = pending {
            let result = if connected && status == 0 {
                Ok(())
            } else {
                Err(BluetoothError::ConnectionFailed(format!(
                    "onConnectionStateChange failed: connected={connected}, status={status}"
                )))
            };
            let _ = tx.send(result);
        }
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onServicesDiscoveredNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    _device_address: JString<'caller>,
    payload: JString<'caller>,
    status: jni::sys::jint,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<GattCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_gatt_state"));

        let result = if status == 0 {
            let payload = payload.try_to_string(env).unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: decode services payload failed in callback: {error}")
            });
            parse_services_payload(&payload)
        } else {
            Err(BluetoothError::GattError(format!(
                "onServicesDiscovered failed with status {status}"
            )))
        };

        let pending = state
            .discover_services
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: GATT discover slot mutex poisoned: {error}")
            })
            .take();
        if let Some(tx) = pending {
            let _ = tx.send(result);
        } else {
            debug_assert!(
                false,
                "waterkit-bluetooth: discover callback fired without pending receiver"
            );
        }
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicReadNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    _device_address: JString<'caller>,
    service_uuid: JString<'caller>,
    characteristic_uuid: JString<'caller>,
    value: JByteArray<'caller>,
    status: jni::sys::jint,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<GattCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_gatt_state"));

        let service_uuid = service_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode service UUID failed in read callback: {error}")
        });
        let characteristic_uuid = characteristic_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in read callback: {error}"
            )
        });
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

        let pending = state
            .reads
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: GATT read map mutex poisoned: {error}")
            })
            .remove(&key);
        if let Some(tx) = pending {
            let _ = tx.send(result);
        } else {
            debug_assert!(
                false,
                "waterkit-bluetooth: read callback fired without pending receiver"
            );
        }
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicWriteNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    _device_address: JString<'caller>,
    service_uuid: JString<'caller>,
    characteristic_uuid: JString<'caller>,
    status: jni::sys::jint,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<GattCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_gatt_state"));

        let service_uuid = service_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!("waterkit-bluetooth: decode service UUID failed in write callback: {error}")
        });
        let characteristic_uuid = characteristic_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in write callback: {error}"
            )
        });
        let key = (service_uuid, characteristic_uuid);

        let result = if status == 0 {
            Ok(())
        } else {
            Err(BluetoothError::GattError(format!(
                "onCharacteristicWrite failed with status {status}"
            )))
        };

        let pending = state
            .writes
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-bluetooth: GATT write map mutex poisoned: {error}")
            })
            .remove(&key);
        if let Some(tx) = pending {
            let _ = tx.send(result);
        } else {
            debug_assert!(
                false,
                "waterkit-bluetooth: write callback fired without pending receiver"
            );
        }
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_BleGattBridgeCallback_onCharacteristicChangedNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    _device_address: JString<'caller>,
    service_uuid: JString<'caller>,
    characteristic_uuid: JString<'caller>,
    value: JByteArray<'caller>,
) {
    with_callback_env(&mut env, |env| {
        let state: Arc<GattCallbackState> =
            callback_state(env, &callback, jni_str!("waterkit_gatt_state"));

        let service_uuid = service_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode service UUID failed in notification callback: {error}"
            )
        });
        let characteristic_uuid = characteristic_uuid.try_to_string(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode characteristic UUID failed in notification callback: {error}"
            )
        });
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
        Ok(())
    });
}

pub struct BleConnectionInner {
    gatt: Global<JObject<'static>>,
    callback: Global<JObject<'static>>,
    callback_state: Arc<GattCallbackState>,
    closed: bool,
}

impl std::fmt::Debug for BleConnectionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleConnectionInner")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl BleConnectionInner {
    fn callback_state(&self) -> Arc<GattCallbackState> {
        Arc::clone(&self.callback_state)
    }

    fn close_gatt(&mut self) -> Result<(), BluetoothError> {
        if self.closed {
            return Ok(());
        }
        with_android_context(|env, _context| release_callback_state(env, &self.callback))?;
        self.closed = true;
        with_android_context(|env, _context| {
            env.call_method(
                self.gatt.as_obj(),
                jni_str!("disconnect"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!("BluetoothGatt.disconnect failed: {error}"))
            })?;
            env.call_method(self.gatt.as_obj(), jni_str!("close"), jni_sig!("()V"), &[])
                .map_err(|error| {
                    BluetoothError::Platform(format!("BluetoothGatt.close failed: {error}"))
                })?;
            Ok(())
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Connecting GATT on Android is one ordered JNI setup sequence: callback state, Java callback, connect call, and callback await."
    )]
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        match adapter_state().await? {
            AdapterState::PoweredOn => {}
            AdapterState::PoweredOff => return Err(BluetoothError::PoweredOff),
            AdapterState::Unavailable | AdapterState::Unknown => {
                return Err(BluetoothError::NotAvailable);
            }
            AdapterState::Unauthorized => return Err(BluetoothError::PermissionDenied),
        }

        let callback_state = Arc::new(GattCallbackState::new());
        let callback_state_handle = callback_state_handle(&callback_state)?;
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();
        {
            let mut connect_slot = callback_state.connect.lock().map_err(|error| {
                BluetoothError::Platform(format!("GATT connect slot mutex poisoned: {error}"))
            })?;
            *connect_slot = Some(connect_tx);
        }
        let setup = with_android_context(|env, context| {
            let loader = init_dex(env, context)?;
            register_callback_natives(env, &loader)?;
            let helper_class = get_helper_class(env, &loader)?;
            let callback_class =
                load_class(env, &loader, "waterkit.bluetooth.BleGattBridgeCallback")?;
            let callback = env
                .new_object(callback_class, jni_sig!("()V"), &[])
                .map_err(|error| {
                    BluetoothError::Platform(format!("new BleGattBridgeCallback failed: {error}"))
                })?;
            env.set_field(
                &callback,
                jni_str!("waterkit_gatt_state"),
                jni_sig!("J"),
                JValue::Long(callback_state_handle),
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "set BleGattBridgeCallback.waterkit_gatt_state failed: {error}"
                ))
            })?;
            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::Platform(format!("new_global_ref callback failed: {error}"))
            })?;

            let address = env.new_string(device_id.as_str()).map_err(|error| {
                BluetoothError::Platform(format!("new_string address failed: {error}"))
            })?;
            let gatt = env
                .call_static_method(
                    &helper_class,
                    jni_str!("connectGatt"),
                    jni_sig!(
                        "(Landroid/content/Context;Ljava/lang/String;Landroid/bluetooth/BluetoothGattCallback;)Landroid/bluetooth/BluetoothGatt;"
                    ),
                    &[
                        JValue::Object(context),
                        JValue::Object(&address),
                        JValue::Object(callback.as_obj()),
                    ],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!("BluetoothHelper.connectGatt failed: {error}"))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::Platform(format!("connectGatt return decode failed: {error}"))
                })?;
            if gatt.is_null() {
                release_callback_state(env, &callback)?;
                return Err(BluetoothError::DeviceNotFound(
                    device_id.as_str().to_string(),
                ));
            }

            let gatt = match env.new_global_ref(&gatt) {
                Ok(global) => global,
                Err(error) => {
                    let _ = env.call_method(&gatt, jni_str!("disconnect"), jni_sig!("()V"), &[]);
                    let _ = env.call_method(&gatt, jni_str!("close"), jni_sig!("()V"), &[]);
                    release_callback_state(env, &callback)?;
                    return Err(BluetoothError::Platform(format!(
                        "new_global_ref gatt failed: {error}"
                    )));
                }
            };
            Ok((gatt, callback))
        });

        let (gatt, callback) = match setup {
            Ok(values) => values,
            Err(error) => return Err(error),
        };

        let connect_result = connect_rx.await.map_err(|error| {
            BluetoothError::ConnectionFailed(format!("connect callback dropped: {error}"))
        })?;

        if let Err(error) = connect_result {
            let mut connection = Self {
                gatt,
                callback,
                callback_state,
                closed: false,
            };
            let _ = connection.close_gatt();
            return Err(error);
        }

        Ok(Self {
            gatt,
            callback,
            callback_state,
            closed: false,
        })
    }

    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        let state = self.callback_state();
        let (tx, rx) = oneshot::channel::<Result<Vec<GattService>, BluetoothError>>();
        {
            let mut discover_slot = state.discover_services.lock().map_err(|error| {
                BluetoothError::Platform(format!(
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
            env.call_method(
                self.gatt.as_obj(),
                jni_str!("discoverServices"),
                jni_sig!("()Z"),
                &[],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!("BluetoothGatt.discoverServices failed: {error}"))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.discoverServices return decode failed: {error}"
                ))
            })
        })?;
        if !started {
            state
                .discover_services
                .lock()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "GATT discover_services slot mutex poisoned: {error}"
                    ))
                })?
                .take();
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
        let state = self.callback_state();
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = oneshot::channel::<Result<Vec<u8>, BluetoothError>>();
        {
            let mut reads = state.reads.lock().map_err(|error| {
                BluetoothError::Platform(format!("GATT read map mutex poisoned: {error}"))
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
                jni_str!("readCharacteristic"),
                jni_sig!("(Landroid/bluetooth/BluetoothGattCharacteristic;)Z"),
                &[JValue::Object(&characteristic_obj)],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.readCharacteristic failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.readCharacteristic return decode failed: {error}"
                ))
            })
        })?;

        if !started {
            state
                .reads
                .lock()
                .map_err(|error| {
                    BluetoothError::Platform(format!("GATT read map mutex poisoned: {error}"))
                })?
                .remove(&key);
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
        let state = self.callback_state();
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = oneshot::channel::<Result<(), BluetoothError>>();
        {
            let mut writes = state.writes.lock().map_err(|error| {
                BluetoothError::Platform(format!("GATT write map mutex poisoned: {error}"))
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
                BluetoothError::Platform(format!("byte_array_from_slice failed: {error}"))
            })?;
            let set_value = env
                .call_method(
                    &characteristic_obj,
                    jni_str!("setValue"),
                    jni_sig!("([B)Z"),
                    &[JValue::Object(&JObject::from(payload))],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattCharacteristic.setValue failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattCharacteristic.setValue return decode failed: {error}"
                    ))
                })?;
            if !set_value {
                return Ok(false);
            }
            env.call_method(
                self.gatt.as_obj(),
                jni_str!("writeCharacteristic"),
                jni_sig!("(Landroid/bluetooth/BluetoothGattCharacteristic;)Z"),
                &[JValue::Object(&characteristic_obj)],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.writeCharacteristic failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.writeCharacteristic return decode failed: {error}"
                ))
            })
        })?;

        if !started {
            state
                .writes
                .lock()
                .map_err(|error| {
                    BluetoothError::Platform(format!("GATT write map mutex poisoned: {error}"))
                })?
                .remove(&key);
            return Err(BluetoothError::GattError(
                "BluetoothGatt.writeCharacteristic returned false".into(),
            ));
        }

        rx.await.map_err(|error| {
            BluetoothError::GattError(format!("writeCharacteristic callback dropped: {error}"))
        })?
    }

    #[allow(
        clippy::too_many_lines,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "Android notification subscription is one JNI descriptor transaction; the API is async to match callback-based GATT operations."
    )]
    pub async fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        let state = self.callback_state();
        let key = (service.0.clone(), characteristic.0.clone());
        let (tx, rx) = async_channel::unbounded();
        {
            let mut subscriptions = state.subscriptions.lock().map_err(|error| {
                BluetoothError::Platform(format!("GATT subscription map mutex poisoned: {error}"))
            })?;
            subscriptions.insert(key.clone(), tx);
        }

        let enabled = with_android_context(|env, _context| {
            let characteristic_obj =
                jni_characteristic(env, self.gatt.as_obj(), &service.0, &characteristic.0)?;
            let notification_set = env
                .call_method(
                    self.gatt.as_obj(),
                    jni_str!("setCharacteristicNotification"),
                    jni_sig!("(Landroid/bluetooth/BluetoothGattCharacteristic;Z)Z"),
                    &[JValue::Object(&characteristic_obj), JValue::Bool(true)],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGatt.setCharacteristicNotification failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
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
                    jni_str!("getDescriptor"),
                    jni_sig!("(Ljava/util/UUID;)Landroid/bluetooth/BluetoothGattDescriptor;"),
                    &[JValue::Object(&descriptor_uuid)],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattCharacteristic.getDescriptor failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattCharacteristic.getDescriptor return decode failed: {error}"
                    ))
                })?;
            if descriptor.is_null() {
                return Err(BluetoothError::GattError(
                    "CCCD descriptor not found for characteristic".into(),
                ));
            }

            let descriptor_class = env
                .find_class(jni_str!("android/bluetooth/BluetoothGattDescriptor"))
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "find BluetoothGattDescriptor class failed: {error}"
                    ))
                })?;
            let enable_value = env
                .get_static_field(
                    descriptor_class,
                    jni_str!("ENABLE_NOTIFICATION_VALUE"),
                    jni_sig!("[B"),
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "read ENABLE_NOTIFICATION_VALUE failed: {error}"
                    ))
                })?
                .l()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "decode ENABLE_NOTIFICATION_VALUE failed: {error}"
                    ))
                })?;
            let set_descriptor = env
                .call_method(
                    &descriptor,
                    jni_str!("setValue"),
                    jni_sig!("([B)Z"),
                    &[JValue::Object(&enable_value)],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattDescriptor.setValue failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothGattDescriptor.setValue return decode failed: {error}"
                    ))
                })?;
            if !set_descriptor {
                return Ok(false);
            }

            env.call_method(
                self.gatt.as_obj(),
                jni_str!("writeDescriptor"),
                jni_sig!("(Landroid/bluetooth/BluetoothGattDescriptor;)Z"),
                &[JValue::Object(&descriptor)],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!("BluetoothGatt.writeDescriptor failed: {error}"))
            })?
            .z()
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothGatt.writeDescriptor return decode failed: {error}"
                ))
            })
        })?;

        if !enabled {
            state
                .subscriptions
                .lock()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "GATT subscription map mutex poisoned: {error}"
                    ))
                })?
                .remove(&key);
            return Err(BluetoothError::GattError(
                "enable characteristic notifications failed".into(),
            ));
        }

        Ok(rx)
    }

    #[allow(
        clippy::unused_async,
        reason = "The public connection API is async across platform backends; Android close is synchronous."
    )]
    pub async fn disconnect(mut self) {
        let _ = self.close_gatt();
    }
}

impl Drop for BleConnectionInner {
    fn drop(&mut self) {
        let _ = self.close_gatt();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_bluetooth_ClassicDiscoveryBridgeCallback_onDeviceFoundNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    device_address: JString<'caller>,
    device_name: JObject<'caller>,
    major_device_class: jni::sys::jint,
    is_paired: jni::sys::jboolean,
) {
    with_callback_env(&mut env, |env| {
        let sender: Arc<async_channel::Sender<ClassicDevice>> =
            callback_state(env, &callback, jni_str!("waterkit_classic_discovery_state"));

        let device_address = device_address.try_to_string(env).unwrap_or_else(|error| {
            panic!(
                "waterkit-bluetooth: decode deviceAddress failed in classic discovery callback: {error}"
            )
        });
        let device_name = if device_name.is_null() {
            None
        } else {
            let value = env
                .as_cast::<JString>(&device_name)
                .and_then(|value| value.try_to_string(env))
                .unwrap_or_else(|error| {
                    panic!(
                        "waterkit-bluetooth: decode deviceName failed in classic discovery callback: {error}"
                    )
                });
            if value.is_empty() { None } else { Some(value) }
        };

        if let Err(error) = sender.try_send(ClassicDevice {
            device: BluetoothDevice {
                id: DeviceId::new(device_address),
                name: device_name,
                rssi: None,
                is_connected: false,
            },
            device_class: major_device_class.cast_unsigned(),
            is_paired,
        }) {
            debug_assert!(
                false,
                "waterkit-bluetooth: dropping classic discovery result because receiver is closed: {error}"
            );
        }
        Ok(())
    });
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

    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    pub async fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        {
            let session = self.discovery_session.lock().map_err(|error| {
                BluetoothError::Platform(format!(
                    "classic discovery session mutex poisoned: {error}"
                ))
            })?;
            if session.is_some() {
                return Err(BluetoothError::Platform(
                    "classic discovery already active".into(),
                ));
            }
        }

        let (tx, rx) = async_channel::unbounded();
        let callback_state = Arc::new(tx);
        let callback_state_handle = callback_state_handle(&callback_state)?;
        let session = with_android_context(|env, context| {
            let loader = init_dex(env, context)?;
            register_callback_natives(env, &loader)?;
            let helper_class = get_helper_class(env, &loader)?;
            let callback_class = load_class(
                env,
                &loader,
                "waterkit.bluetooth.ClassicDiscoveryBridgeCallback",
            )?;
            let callback = env
                .new_object(callback_class, jni_sig!("()V"), &[])
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "new ClassicDiscoveryBridgeCallback failed: {error}"
                    ))
                })?;
            env.set_field(
                &callback,
                jni_str!("waterkit_classic_discovery_state"),
                jni_sig!("J"),
                JValue::Long(callback_state_handle),
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "set waterkit_classic_discovery_state failed: {error}"
                ))
            })?;
            let callback = env.new_global_ref(callback).map_err(|error| {
                BluetoothError::Platform(format!("new_global_ref callback failed: {error}"))
            })?;

            let started = env
                .call_static_method(
                    &helper_class,
                    jni_str!("startClassicDiscovery"),
                    jni_sig!(
                        "(Landroid/content/Context;Lwaterkit/bluetooth/ClassicDiscoveryCallback;)Z"
                    ),
                    &[JValue::Object(context), JValue::Object(callback.as_obj())],
                )
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "BluetoothHelper.startClassicDiscovery failed: {error}"
                    ))
                })?
                .z()
                .map_err(|error| {
                    BluetoothError::Platform(format!(
                        "startClassicDiscovery return decode failed: {error}"
                    ))
                })?;
            if !started {
                release_callback_state(env, &callback)?;
                return Err(BluetoothError::GattError(
                    "BluetoothAdapter.startDiscovery returned false".into(),
                ));
            }

            Ok(ClassicDiscoverySession {
                callback,
                _callback_state: Arc::clone(&callback_state),
            })
        })?;

        *self.discovery_session.lock().map_err(|error| {
            BluetoothError::Platform(format!("classic discovery session mutex poisoned: {error}"))
        })? = Some(session);
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

        with_android_context(|env, context| {
            release_callback_state(env, &session.callback)?;
            let loader = init_dex(env, context)?;
            let helper_class = get_helper_class(env, &loader)?;
            env.call_static_method(
                &helper_class,
                jni_str!("stopClassicDiscovery"),
                jni_sig!("(Landroid/content/Context;)V"),
                &[JValue::Object(context)],
            )
            .map_err(|error| {
                BluetoothError::Platform(format!(
                    "BluetoothHelper.stopClassicDiscovery failed: {error}"
                ))
            })?;
            Ok(())
        })
    }

    #[allow(clippy::unused_async)]
    pub async fn stop_discovery(&self) {
        let _ = self.stop_discovery_impl();
    }

    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        future::ready(with_android_context(get_paired_devices_with_context)).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Opening Android SPP requires one ordered JNI/socket setup plus worker-thread ownership transfer."
    )]
    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        let device_id = device_id.as_str().to_string();
        let service_uuid = uuid.as_str().to_string();
        let (command_tx, command_rx) = async_channel::unbounded();
        let (connect_tx, connect_rx) = oneshot::channel::<Result<(), BluetoothError>>();

        let worker = std::thread::Builder::new()
            .name("waterkit-spp-android".to_owned())
            .spawn(move || {
                let socket = with_android_context(|env, context| {
                    let loader = init_dex(env, context)?;
                    let helper_class = get_helper_class(env, &loader)?;
                    let device_id = env.new_string(device_id).map_err(|error| {
                        BluetoothError::Platform(format!("new_string device_id failed: {error}"))
                    })?;
                    let service_uuid = env.new_string(service_uuid).map_err(|error| {
                        BluetoothError::Platform(format!(
                            "new_string service_uuid failed: {error}"
                        ))
                    })?;
                    let socket = env
                        .call_static_method(
                            &helper_class,
                            jni_str!("connectSpp"),
                            jni_sig!(
                                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Landroid/bluetooth/BluetoothSocket;"
                            ),
                            &[
                                JValue::Object(context),
                                JValue::Object(&device_id),
                                JValue::Object(&service_uuid),
                            ],
                        )
                        .map_err(|error| {
                            BluetoothError::Platform(format!(
                                "BluetoothHelper.connectSpp failed: {error}"
                            ))
                        })?
                        .l()
                        .map_err(|error| {
                            BluetoothError::Platform(format!(
                                "connectSpp return decode failed: {error}"
                            ))
                        })?;
                    if socket.is_null() {
                        return Err(BluetoothError::ConnectionFailed(
                            "connectSpp returned null socket".into(),
                        ));
                    }
                    env.new_global_ref(socket).map_err(|error| {
                        BluetoothError::Platform(format!(
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
                                let loader = init_dex(env, context)?;
                                let helper_class = get_helper_class(env, &loader)?;
                                let max_bytes = i32::try_from(max_bytes).map_err(|_| {
                                    BluetoothError::Platform(format!(
                                        "SPP read size exceeds i32: {max_bytes}"
                                    ))
                                })?;
                                let bytes = env
                                    .call_static_method(
                                        &helper_class,
                                        jni_str!("readSpp"),
                                        jni_sig!("(Landroid/bluetooth/BluetoothSocket;I)[B"),
                                        &[
                                            JValue::Object(socket.as_obj()),
                                            JValue::Int(max_bytes),
                                        ],
                                    )
                                    .map_err(|error| {
                                        BluetoothError::Platform(format!(
                                            "BluetoothHelper.readSpp failed: {error}"
                                        ))
                                    })?
                                    .l()
                                    .map_err(|error| {
                                        BluetoothError::Platform(format!(
                                            "readSpp return decode failed: {error}"
                                        ))
                                    })?;
                                if bytes.is_null() {
                                    return Err(BluetoothError::ConnectionFailed(
                                        "SPP stream closed".into(),
                                    ));
                                }
                                let bytes = JByteArray::cast_local(env, bytes).map_err(|error| {
                                    BluetoothError::Platform(format!(
                                        "cast readSpp byte array failed: {error}"
                                    ))
                                })?;
                                env.convert_byte_array(bytes).map_err(|error| {
                                    BluetoothError::Platform(format!(
                                        "decode readSpp byte array failed: {error}"
                                    ))
                                })
                            });
                            let _ = tx.send(result);
                        }
                        SppCommand::Write { data, tx } => {
                            let result = with_android_context(|env, context| {
                                let loader = init_dex(env, context)?;
                                let helper_class = get_helper_class(env, &loader)?;
                                let payload = env.byte_array_from_slice(&data).map_err(|error| {
                                    BluetoothError::Platform(format!(
                                        "byte_array_from_slice failed in writeSpp: {error}"
                                    ))
                                })?;
                                let written = env
                                    .call_static_method(
                                        &helper_class,
                                        jni_str!("writeSpp"),
                                        jni_sig!("(Landroid/bluetooth/BluetoothSocket;[B)I"),
                                        &[
                                            JValue::Object(socket.as_obj()),
                                            JValue::Object(&JObject::from(payload)),
                                        ],
                                    )
                                    .map_err(|error| {
                                        BluetoothError::Platform(format!(
                                            "BluetoothHelper.writeSpp failed: {error}"
                                        ))
                                    })?
                                    .i()
                                    .map_err(|error| {
                                        BluetoothError::Platform(format!(
                                            "writeSpp return decode failed: {error}"
                                        ))
                                    })?;
                                usize::try_from(written).map_err(|_| {
                                    BluetoothError::Platform(format!(
                                        "writeSpp returned negative byte count: {written}"
                                    ))
                                })
                            });
                            let _ = tx.send(result);
                        }
                        SppCommand::Close { tx } => {
                            let _ = with_android_context(|env, context| {
                                let loader = init_dex(env, context)?;
                                let helper_class = get_helper_class(env, &loader)?;
                                env.call_static_method(
                                    &helper_class,
                                    jni_str!("closeSpp"),
                                    jni_sig!("(Landroid/bluetooth/BluetoothSocket;)V"),
                                    &[JValue::Object(socket.as_obj())],
                                )
                                .map_err(|error| {
                                    BluetoothError::Platform(format!(
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
                    let loader = init_dex(env, context)?;
                    let helper_class = get_helper_class(env, &loader)?;
                    env.call_static_method(
                        &helper_class,
                        jni_str!("closeSpp"),
                        jni_sig!("(Landroid/bluetooth/BluetoothSocket;)V"),
                        &[JValue::Object(socket.as_obj())],
                    )
                    .map_err(|error| {
                        BluetoothError::Platform(format!(
                            "BluetoothHelper.closeSpp failed during worker shutdown: {error}"
                        ))
                    })?;
                    Ok(())
                });
            })
            .map_err(|error| {
                BluetoothError::Platform(format!("spawn SPP worker failed: {error}"))
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
    use super::{get_helper_class, init_dex};
    use crate::AdapterState;
    use crate::BluetoothError;
    use jni::objects::{JObject, JValue};
    use jni::{Env, jni_sig, jni_str};

    /// Get adapter state with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn get_adapter_state(
        env: &mut Env<'_>,
        context: &JObject,
    ) -> Result<AdapterState, BluetoothError> {
        let loader = init_dex(env, context)?;
        let helper_class = get_helper_class(env, &loader)?;
        let state = env
            .call_static_method(
                &helper_class,
                jni_str!("getAdapterState"),
                jni_sig!("(Landroid/content/Context;)I"),
                &[JValue::Object(context)],
            )
            .map_err(|e| BluetoothError::Platform(format!("getAdapterState: {e}")))?
            .i()
            .map_err(|e| BluetoothError::Platform(format!("return: {e}")))?;
        match state {
            0 => Ok(AdapterState::PoweredOff),
            1 => Ok(AdapterState::PoweredOn),
            2 => Ok(AdapterState::Unavailable),
            3 => Ok(AdapterState::Unauthorized),
            _ => Ok(AdapterState::Unknown),
        }
    }
}
