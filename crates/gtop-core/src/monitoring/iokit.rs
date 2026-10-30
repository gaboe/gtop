//! Apple GPU and internal SSD readings from the IOKit registry
//!
//! Every GPU registers an `IOAccelerator` service. Its `PerformanceStatistics`
//! dictionary holds the utilization and memory figures Activity Monitor shows,
//! and each child user client records the GPU time its owning process used.
//! None of this needs elevated privileges.

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_kit::{
    io_iterator_t, io_object_t, kIOReturnSuccess, kIOServicePlane, IOIteratorNext, IOObjectRelease,
    IORegistryEntryCreateCFProperty, IORegistryEntryGetChildIterator,
    IORegistryEntryGetParentEntry, IOServiceGetMatchingServices, IOServiceMatching,
};
use std::collections::HashMap;

/// An IOKit object handle, released on drop
#[derive(Debug)]
struct Object(io_object_t);

impl Drop for Object {
    fn drop(&mut self) {
        IOObjectRelease(self.0);
    }
}

impl Object {
    fn next(iterator: &Object) -> Option<Object> {
        let object = IOIteratorNext(iterator.0);
        (object != 0).then_some(Object(object))
    }

    fn property(&self, key: &'static str) -> Option<CFRetained<CFType>> {
        let key = CFString::from_static_str(key);
        unsafe { IORegistryEntryCreateCFProperty(self.0, Some(&key), None, 0) }
    }

    fn children(&self) -> Vec<Object> {
        let mut plane: [std::ffi::c_char; 128] = [0; 128];
        for (dst, src) in plane.iter_mut().zip(kIOServicePlane.to_bytes()) {
            *dst = *src as _;
        }
        let mut iterator: io_iterator_t = 0;
        if unsafe { IORegistryEntryGetChildIterator(self.0, &mut plane, &mut iterator) }
            != kIOReturnSuccess
        {
            return Vec::new();
        }
        let iterator = Object(iterator);
        std::iter::from_fn(|| Object::next(&iterator)).collect()
    }
}

// An IOKit handle is a Mach port name, valid from any thread
unsafe impl Send for Object {}

/// One GPU as registered with IOKit
#[derive(Debug)]
pub struct Accelerator(Object);

/// Live readings of one GPU
pub struct AcceleratorStats {
    /// Device utilization as percentage (0-100)
    pub utilization: Option<f32>,
    /// Memory the GPU currently uses, in bytes
    pub memory_used: Option<u64>,
}

impl Accelerator {
    /// Lists every GPU in the machine
    pub fn all() -> Vec<Accelerator> {
        let Some(matching) = (unsafe { IOServiceMatching(c"IOAccelerator".as_ptr()) }) else {
            return Vec::new();
        };
        let matching = CFRetained::<CFDictionary>::from(&matching);
        let mut iterator: io_iterator_t = 0;
        if unsafe { IOServiceGetMatchingServices(0, Some(matching), &mut iterator) }
            != kIOReturnSuccess
        {
            return Vec::new();
        }
        let iterator = Object(iterator);
        std::iter::from_fn(|| Object::next(&iterator))
            .map(Accelerator)
            .collect()
    }

    /// Model name, such as "Apple M3 Max (30-core GPU)"
    pub fn name(&self) -> String {
        let model = self
            .0
            .property("model")
            .and_then(|value| value.downcast::<CFString>().ok())
            .map(|model| model.to_string())
            .unwrap_or_else(|| "GPU".to_string());
        match self.number("gpu-core-count") {
            Some(cores) => format!("{model} ({cores}-core GPU)"),
            None => model,
        }
    }

    /// Reads the current utilization and memory use
    pub fn stats(&self) -> AcceleratorStats {
        let statistics = self
            .0
            .property("PerformanceStatistics")
            .and_then(|value| value.downcast::<CFDictionary>().ok());
        let read = |key: &'static str| {
            statistics
                .as_ref()
                .and_then(|statistics| dictionary_number(statistics, key))
        };
        AcceleratorStats {
            utilization: read("Device Utilization %").map(|value| value as f32),
            memory_used: read("In use system memory").map(|value| value as u64),
        }
    }

    /// Sums the GPU time, in nanoseconds, each process has used so far
    pub fn gpu_time_per_process(&self, totals: &mut HashMap<u32, u64>) {
        for client in self.0.children() {
            let Some(pid) = client
                .property("IOUserClientCreator")
                .and_then(|value| value.downcast::<CFString>().ok())
                .and_then(|creator| parse_creator_pid(&creator.to_string()))
            else {
                continue;
            };
            let Some(usage) = client
                .property("AppUsage")
                .and_then(|value| value.downcast::<CFArray>().ok())
            else {
                continue;
            };
            let used: i64 = (0..usage.count())
                .filter_map(|index| {
                    let entry = unsafe { usage.as_opaque().value_at_index(index) };
                    let entry = unsafe { &*(entry as *const CFDictionary) };
                    dictionary_number(entry, "accumulatedGPUTime")
                })
                .map(|value| value as i64)
                .sum();
            *totals.entry(pid).or_default() += used.max(0) as u64;
        }
    }

    fn number(&self, key: &'static str) -> Option<i64> {
        self.0
            .property(key)
            .and_then(|value| value.downcast::<CFNumber>().ok())
            .and_then(|number| number.as_i64())
    }
}

fn dictionary_number(dictionary: &CFDictionary, key: &'static str) -> Option<f64> {
    let key = CFString::from_static_str(key);
    let dictionary = unsafe { dictionary.cast_unchecked::<CFString, CFType>() };
    dictionary
        .get(&key)
        .and_then(|value| value.downcast::<CFNumber>().ok())
        .and_then(|number| number.as_f64())
}

/// Reads the pid out of a creator string such as "pid 620, WindowServer"
fn parse_creator_pid(creator: &str) -> Option<u32> {
    creator
        .strip_prefix("pid ")?
        .split(',')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "headless SSD counter CPU-cost probe"]
    fn storage_counter_read_cost() {
        let drivers = StorageDrivers::new();
        let cpu = || {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            assert_eq!(
                unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
                0
            );
            let usage = unsafe { usage.assume_init() };
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1e6
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64
        };
        let start = cpu();
        let wall = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(std::hint::black_box(drivers.totals()).is_some());
        }
        println!(
            "1000 SSD reads: cpu_us={}, wall_us={}",
            cpu() - start,
            wall.elapsed().as_micros()
        );
        for index in 0..10 {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let start = cpu();
            let wall = std::time::Instant::now();
            assert!(std::hint::black_box(drivers.totals()).is_some());
            println!(
                "cold SSD read {index}: cpu_us={}, wall_us={}",
                cpu() - start,
                wall.elapsed().as_micros()
            );
        }
    }

    #[test]
    fn parses_creator_pid() {
        assert_eq!(parse_creator_pid("pid 620, WindowServer"), Some(620));
        assert_eq!(parse_creator_pid("WindowServer"), None);
    }
}

/// Cached internal SSD drivers. Each driver counts physical I/O once, below APFS.
#[derive(Debug)]
pub struct StorageDrivers(Vec<Object>);
impl StorageDrivers {
    pub fn new() -> Self {
        let Some(matching) = (unsafe { IOServiceMatching(c"IOBlockStorageDriver".as_ptr()) })
        else {
            return Self(Vec::new());
        };
        let matching = CFRetained::<CFDictionary>::from(&matching);
        let mut iterator = 0;
        if unsafe { IOServiceGetMatchingServices(0, Some(matching), &mut iterator) }
            != kIOReturnSuccess
        {
            return Self(Vec::new());
        }
        let iterator = Object(iterator);
        let drivers = std::iter::from_fn(|| Object::next(&iterator))
            .filter(|driver| {
                let mut parent = 0;
                let mut plane = [0; 128];
                for (dst, src) in plane.iter_mut().zip(kIOServicePlane.to_bytes()) {
                    *dst = *src as _;
                }
                if unsafe { IORegistryEntryGetParentEntry(driver.0, &mut plane, &mut parent) }
                    != kIOReturnSuccess
                {
                    return false;
                }
                let parent = Object(parent);
                let matches = |property, key, expected| {
                    parent
                        .property(property)
                        .and_then(|v| v.downcast::<CFDictionary>().ok())
                        .is_some_and(|d| {
                            let d = unsafe { d.cast_unchecked::<CFString, CFType>() };
                            d.get(&CFString::from_static_str(key))
                                .and_then(|v| v.downcast::<CFString>().ok())
                                .is_some_and(|v| v.to_string() == expected)
                        })
                };
                matches("Device Characteristics", "Medium Type", "Solid State")
                    && matches(
                        "Protocol Characteristics",
                        "Physical Interconnect Location",
                        "Internal",
                    )
            })
            .collect();
        Self(drivers)
    }

    pub fn totals(&self) -> Option<(u64, u64)> {
        if self.0.is_empty() {
            return None;
        }
        self.0
            .iter()
            .try_fold((0u64, 0u64), |(read, written), driver| {
                let statistics = driver
                    .property("Statistics")?
                    .downcast::<CFDictionary>()
                    .ok()?;
                let number = |key| {
                    let d = unsafe { statistics.cast_unchecked::<CFString, CFType>() };
                    let n = d
                        .get(&CFString::from_static_str(key))?
                        .downcast::<CFNumber>()
                        .ok()?
                        .as_i64()?;
                    u64::try_from(n).ok()
                };
                Some((
                    read.checked_add(number("Bytes (Read)")?)?,
                    written.checked_add(number("Bytes (Write)")?)?,
                ))
            })
    }
}
