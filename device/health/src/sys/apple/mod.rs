//! `HealthKit` realisation via `objc2-health-kit`.
//!
//! `HKHealthStore` is thread-safe and every operation is driven by its own
//! completion handler on a `HealthKit` background queue, so nothing here hops
//! onto the main queue and no Objective-C object crosses a thread boundary:
//! the store, the sample type, and the query/sample all live inside the
//! `async fn` after the last `.await`, and only typed `HealthSample` values
//! cross the oneshot channel.

use std::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSArray, NSDate, NSError};
use objc2_health_kit::{
    HKAuthorizationStatus, HKCategorySample, HKCategoryTypeIdentifierSleepAnalysis,
    HKCategoryValueSleepAnalysis, HKHealthStore, HKMetricPrefix, HKObjectQueryNoLimit,
    HKObjectType, HKQuantity, HKQuantitySample, HKQuantityType,
    HKQuantityTypeIdentifierActiveEnergyBurned, HKQuantityTypeIdentifierBodyMass,
    HKQuantityTypeIdentifierDistanceWalkingRunning, HKQuantityTypeIdentifierHeartRate,
    HKQuantityTypeIdentifierHeight, HKQuantityTypeIdentifierOxygenSaturation,
    HKQuantityTypeIdentifierStepCount, HKQuery, HKQueryOptions, HKSample, HKSampleQuery,
    HKSampleType, HKUnit,
};
use waterkit_core::Timestamp;

use crate::{HealthDataType, HealthError, HealthSample};

pub fn is_available() -> bool {
    // SAFETY: `isHealthDataAvailable` is a class method safe to call from any thread.
    unsafe { HKHealthStore::isHealthDataAvailable() }
}

/// The `HKSampleType` matching `data_type` (a quantity type for every variant
/// except `Sleep`, which is a category type).
fn sample_type(data_type: HealthDataType) -> Retained<HKSampleType> {
    let ty = match data_type {
        HealthDataType::Sleep => unsafe {
            // SAFETY: `categoryTypeForIdentifier` is safe; the identifier static
            // always resolves on a system that links HealthKit.
            HKObjectType::categoryTypeForIdentifier(HKCategoryTypeIdentifierSleepAnalysis)
        }
        .map(Retained::into_super),
        _ => quantity_type(data_type).map(Retained::into_super),
    };
    ty.expect("every HealthDataType identifier is a real HealthKit type")
}

fn quantity_type(data_type: HealthDataType) -> Option<Retained<HKQuantityType>> {
    // SAFETY: the `HKQuantityTypeIdentifier` extern statics are immutable
    // string constants published by HealthKit.
    let identifier = unsafe {
        match data_type {
            HealthDataType::Steps => HKQuantityTypeIdentifierStepCount,
            HealthDataType::HeartRate => HKQuantityTypeIdentifierHeartRate,
            HealthDataType::ActiveEnergy => HKQuantityTypeIdentifierActiveEnergyBurned,
            HealthDataType::Distance => HKQuantityTypeIdentifierDistanceWalkingRunning,
            HealthDataType::Weight => HKQuantityTypeIdentifierBodyMass,
            HealthDataType::Height => HKQuantityTypeIdentifierHeight,
            HealthDataType::BloodOxygen => HKQuantityTypeIdentifierOxygenSaturation,
            HealthDataType::Sleep => return None,
        }
    };
    // SAFETY: `quantityTypeForIdentifier` is safe.
    unsafe { HKObjectType::quantityTypeForIdentifier(identifier) }
}

/// The `HKUnit` and unit label used for a quantity `data_type`, matching the
/// units the Android backend reports (`count`, `bpm`, `kcal`, `m`, `kg`, `%`).
fn unit(data_type: HealthDataType) -> (Retained<HKUnit>, &'static str) {
    // SAFETY: unit factories are pure constructors safe on any thread.
    unsafe {
        match data_type {
            HealthDataType::Steps => (HKUnit::countUnit(), "count"),
            HealthDataType::HeartRate => (
                HKUnit::countUnit().unitDividedByUnit(&HKUnit::minuteUnit()),
                "bpm",
            ),
            HealthDataType::ActiveEnergy => (HKUnit::kilocalorieUnit(), "kcal"),
            HealthDataType::Distance | HealthDataType::Height => (HKUnit::meterUnit(), "m"),
            HealthDataType::Weight => {
                (HKUnit::gramUnitWithMetricPrefix(HKMetricPrefix::Kilo), "kg")
            }
            HealthDataType::BloodOxygen => (HKUnit::percentUnit(), "%"),
            HealthDataType::Sleep => unreachable!("sleep is a category type"),
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Unix seconds fit an f64 exactly for any date EventKit handles"
)]
fn ns_date(timestamp: &Timestamp) -> Retained<NSDate> {
    let seconds = (timestamp.as_second() as f64).mul_add(
        1.0,
        f64::from(timestamp.subsec_nanosecond()) / 1_000_000_000.0,
    );
    NSDate::dateWithTimeIntervalSince1970(seconds)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "HealthKit dates are within Timestamp range; the fractional part is < 1e9"
)]
fn timestamp(date: &NSDate) -> Timestamp {
    let seconds = date.timeIntervalSince1970();
    let whole = seconds.trunc() as i64;
    let nanos = (seconds.fract() * 1_000_000_000.0) as i32;
    Timestamp::new(whole, nanos).expect("HealthKit sample dates are within Timestamp range")
}

fn map_sample(data_type: HealthDataType, sample: &HKSample) -> HealthSample {
    let (value, unit_label) = if data_type == HealthDataType::Sleep {
        // Category sample: report the duration in seconds, same convention as
        // the Android backend's SleepSessionRecord mapping.
        (
            unsafe {
                // SAFETY: `startDate`/`endDate` are read-only accessors on a
                // live sample.
                sample.endDate().timeIntervalSince1970()
                    - sample.startDate().timeIntervalSince1970()
            },
            "s",
        )
    } else {
        let quantity_sample = sample
            .downcast_ref::<HKQuantitySample>()
            .expect("a quantity-type query returns quantity samples");
        let (unit, label) = unit(data_type);
        let mut value = unsafe {
            // SAFETY: `quantity`/`doubleValueForUnit` are safe accessors; the
            // unit matches the data type.
            quantity_sample.quantity().doubleValueForUnit(&unit)
        };
        if data_type == HealthDataType::BloodOxygen {
            // HealthKit reports fractions for `percentUnit`; Health Connect's
            // `Percentage.value` is on the 0-100 scale — normalise to match.
            value *= 100.0;
        }
        (value, label)
    };
    let (start_date, end_date) = unsafe {
        // SAFETY: `startDate`/`endDate` are read-only accessors on a live sample.
        (sample.startDate(), sample.endDate())
    };
    let health_sample = HealthSample::new(
        data_type,
        value,
        unit_label,
        timestamp(&start_date),
        timestamp(&end_date),
    );
    health_sample.source(unsafe {
        // SAFETY: `sourceRevision`/`source`/`name` are read-only accessors on
        // live objects; a stored sample always has a source revision.
        sample.sourceRevision().source().name().to_string()
    })
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    if !is_available() {
        return Err(HealthError::NotAvailable);
    }
    let (tx, rx) = futures::channel::oneshot::channel();
    {
        // Everything non-`Send` (the store, the block, the query) is created
        // inside this scope and only `rx` crosses the `.await`. The copied
        // results block retains its own clone of the store, so HealthKit
        // cannot deallocate it (and possibly cancel the query) before
        // answering.
        let store = unsafe {
            // SAFETY: `HKHealthStore` is thread-safe.
            HKHealthStore::new()
        };
        let ty = sample_type(data_type);
        let start_date = ns_date(&start);
        let end_date = ns_date(&end);
        let predicate = unsafe {
            // SAFETY: `predicateForSamplesWithStartDate:endDate:options:` is safe;
            // strict bounds match the `start..end` contract of `query_samples`.
            HKQuery::predicateForSamplesWithStartDate_endDate_options(
                Some(&start_date),
                Some(&end_date),
                HKQueryOptions::StrictStartDate | HKQueryOptions::StrictEndDate,
            )
        };
        let tx = Mutex::new(Some(tx));
        let block = {
            let store = Retained::clone(&store);
            RcBlock::new(
                move |_query: NonNull<HKSampleQuery>,
                      samples: *mut NSArray<HKSample>,
                      error: *mut NSError| {
                    // Keeping `store` alive until the results handler runs is the
                    // point of this capture.
                    let _ = &store;
                    let result = if !error.is_null() {
                        // SAFETY: `error` is a non-null pointer to a live `NSError`.
                        let description = unsafe { &*error }.localizedDescription().to_string();
                        Err(HealthError::Platform(description))
                    } else if samples.is_null() {
                        Ok(Vec::new())
                    } else {
                        // SAFETY: `samples` is a non-null pointer to a live `NSArray`.
                        let samples: &NSArray<HKSample> = unsafe { &*samples };
                        Ok(samples
                            .iter()
                            .map(|sample| map_sample(data_type, &sample))
                            .collect())
                    };
                    let tx = tx.lock().expect("results tx poisoned").take();
                    if let Some(tx) = tx {
                        let _ = tx.send(result);
                    }
                },
            )
        };
        let query = unsafe {
            // SAFETY: `block` outlives the call and is only invoked once by
            // HealthKit; `tx` inside it is `Send`, satisfying the sendable
            // requirement on results handlers.
            HKSampleQuery::initWithSampleType_predicate_limit_sortDescriptors_resultsHandler(
                HKSampleQuery::alloc(),
                &ty,
                Some(&predicate),
                HKObjectQueryNoLimit,
                None,
                &block,
            )
        };
        unsafe {
            // SAFETY: `query` was just initialised; HealthKit holds it (and the
            // completion handler) until the results handler fires.
            store.executeQuery(&query);
        }
    }
    rx.await.expect("the results handler always answers")
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    if !is_available() {
        return Err(HealthError::NotAvailable);
    }
    let (tx, rx) = futures::channel::oneshot::channel();
    {
        let store = unsafe {
            // SAFETY: as in `query_samples`; the copied save block retains a
            // clone of the store until the completion runs.
            HKHealthStore::new()
        };
        let data_type = sample.data_type();
        let start_date = ns_date(&sample.start());
        let end_date = ns_date(&sample.end());
        let ty = sample_type(data_type);
        // `authorizationStatusForType` only reports the share (write) status;
        // HealthKit intentionally hides the read status.
        if unsafe {
            // SAFETY: `ty` is a live `HKObjectType`.
            store.authorizationStatusForType(&ty)
        } == HKAuthorizationStatus::SharingDenied
        {
            return Err(HealthError::PermissionDenied);
        }
        let object: Retained<objc2_health_kit::HKObject> = if data_type == HealthDataType::Sleep {
            let category_type = unsafe {
                // SAFETY: as in `sample_type`.
                HKObjectType::categoryTypeForIdentifier(HKCategoryTypeIdentifierSleepAnalysis)
            }
            .expect("SleepAnalysis is a real HealthKit category type");
            let category_sample = unsafe {
                // SAFETY: arguments are live and typed correctly; `value` is the
                // "asleep, unspecified" category — matching Android's
                // SleepSessionRecord write.
                HKCategorySample::categorySampleWithType_value_startDate_endDate(
                    &category_type,
                    HKCategoryValueSleepAnalysis::AsleepUnspecified.0,
                    &start_date,
                    &end_date,
                )
            };
            unsafe {
                // SAFETY: `HKCategorySample` inherits `HKObject` — upcast.
                Retained::cast_unchecked(category_sample)
            }
        } else {
            let quantity_type =
                quantity_type(data_type).expect("non-sleep data types are quantity types");
            let (unit, _label) = unit(data_type);
            let quantity = unsafe {
                // SAFETY: pure constructor.
                HKQuantity::quantityWithUnit_doubleValue(&unit, sample.value())
            };
            let quantity_sample = unsafe {
                // SAFETY: arguments are live and typed correctly.
                HKQuantitySample::quantitySampleWithType_quantity_startDate_endDate(
                    &quantity_type,
                    &quantity,
                    &start_date,
                    &end_date,
                )
            };
            unsafe {
                // SAFETY: `HKQuantitySample` inherits `HKObject` — upcast.
                Retained::cast_unchecked(quantity_sample)
            }
        };
        let tx = Mutex::new(Some(tx));
        let block = {
            let store = Retained::clone(&store);
            RcBlock::new(move |success: Bool, error: *mut NSError| {
                // Keeping `store` alive until the completion runs is the point of
                // this capture.
                let _ = &store;
                let result = if !error.is_null() {
                    // SAFETY: `error` is a non-null pointer to a live `NSError`.
                    let description = unsafe { &*error }.localizedDescription().to_string();
                    Err(HealthError::Platform(description))
                } else if success.as_bool() {
                    Ok(())
                } else {
                    Err(HealthError::Platform(
                        "HealthKit save reported failure without an error".into(),
                    ))
                };
                let tx = tx.lock().expect("save tx poisoned").take();
                if let Some(tx) = tx {
                    let _ = tx.send(result);
                }
            })
        };
        unsafe {
            // SAFETY: `object` is a live `HKObject`; `block` is invoked once by
            // HealthKit and `tx` is `Send`.
            store.saveObject_withCompletion(&object, &block);
        }
    }
    rx.await.expect("the save completion always answers")
}
