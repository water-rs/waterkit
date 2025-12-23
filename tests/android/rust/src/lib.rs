//! Android JNI test harness for waterkit crates.
//!
//! This crate is only compiled for Android targets.
//! To build: cargo ndk -t arm64-v8a build -p waterkit-test
//!
//! Add new crate tests by:
//! 1. Adding dependency in Cargo.toml
//! 2. Adding JNI function here
//! 3. Adding matching native declaration in MainActivity.kt

#![cfg(target_os = "android")]
#![allow(non_snake_case)]

use jni::objects::{JClass, JObject};
use jni::sys::{jdoubleArray, jint};
use jni::JNIEnv;

// ============================================================================
// Permission Crate Tests
// ============================================================================

/// Test permission check via JNI.
/// permissionType: 0=Location, 1=Camera, 2=Microphone, etc.
#[no_mangle]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testCheckPermission(
    mut env: JNIEnv,
    _class: JClass,
    activity: JObject,
    permission_type: jint,
) -> jint {
    use waterkit_permission::{Permission, PermissionStatus};
    
    let permission = match permission_type {
        0 => Permission::Location,
        1 => Permission::Camera,
        2 => Permission::Microphone,
        3 => Permission::Photos,
        4 => Permission::Contacts,
        5 => Permission::Calendar,
        _ => return -1,
    };
    
    match waterkit_permission::sys::android::check_with_activity(&mut env, &activity, permission) {
        Ok(status) => match status {
            PermissionStatus::Granted => 3,
            PermissionStatus::Denied => 2,
            PermissionStatus::Restricted => 1,
            PermissionStatus::NotDetermined => 0,
        },
        Err(_) => -1,
    }
}

// ============================================================================
// Location Crate Tests
// ============================================================================

/// Test getting current location via JNI.
/// Returns [success, lat, lng, alt, accuracy, timestamp] or [0.0] on failure.
#[no_mangle]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testGetLocation(
    mut env: JNIEnv,
    _class: JClass,
    context: JObject,
) -> jdoubleArray {
    let create_failure_array = |env: &mut JNIEnv| -> jdoubleArray {
        match env.new_double_array(1) {
            Ok(arr) => {
                let _ = env.set_double_array_region(&arr, 0, &[0.0]);
                arr.into_raw()
            }
            Err(_) => std::ptr::null_mut(),
        }
    };
    
    match waterkit_location::sys::android::get_location_with_context(&mut env, &context) {
        Ok(location) => {
            let result = [
                1.0, // success
                location.latitude,
                location.longitude,
                location.altitude.unwrap_or(0.0),
                location.horizontal_accuracy.unwrap_or(0.0),
                location.timestamp as f64,
            ];
            
            match env.new_double_array(6) {
                Ok(arr) => {
                    let _ = env.set_double_array_region(&arr, 0, &result);
                    arr.into_raw()
                }
                Err(_) => create_failure_array(&mut env),
            }
        }
        Err(_) => create_failure_array(&mut env),
    }
}

// ============================================================================
// Add more crate tests below
// ============================================================================
