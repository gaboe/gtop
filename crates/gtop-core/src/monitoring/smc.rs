//! Read-only AppleSMC GPU temperature discovery. Key names vary by chip.

use objc2_core_foundation::{CFDictionary, CFRetained};
use objc2_io_kit::{
    IOConnectCallStructMethod, IOIteratorNext, IOObjectRelease, IOServiceClose,
    IOServiceGetMatchingServices, IOServiceMatching, IOServiceOpen,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct KeyInfo {
    size: u32,
    kind: u32,
    attributes: u8,
}

#[repr(C)]
#[derive(Default)]
struct Message {
    key: u32,
    version: [u8; 6],
    power_limits: [u32; 4],
    info: KeyInfo,
    result: u8,
    status: u8,
    command: u8,
    index: u32,
    bytes: [u8; 32],
}

#[derive(Debug)]
pub(super) struct GpuTemperature {
    connection: u32,
    keys: Vec<(u32, KeyInfo)>,
}

impl Drop for GpuTemperature {
    fn drop(&mut self) {
        IOServiceClose(self.connection);
    }
}

impl GpuTemperature {
    pub(super) fn new() -> Option<Self> {
        let matching = unsafe { IOServiceMatching(c"AppleSMC".as_ptr()) }?;
        let mut iterator = 0;
        if unsafe {
            IOServiceGetMatchingServices(
                0,
                Some(CFRetained::<CFDictionary>::from(&matching)),
                &mut iterator,
            )
        } != 0
        {
            return None;
        }
        let service = IOIteratorNext(iterator);
        IOObjectRelease(iterator);
        if service == 0 {
            return None;
        }
        extern "C" {
            static mach_task_self_: u32;
        }
        let mut connection = 0;
        let status = unsafe { IOServiceOpen(service, mach_task_self_, 0, &mut connection) };
        IOObjectRelease(service);
        if status != 0 {
            return None;
        }
        let mut sensor = Self {
            connection,
            keys: Vec::new(),
        };
        let count_key = u32::from_be_bytes(*b"#KEY");
        let info = sensor.info(count_key)?;
        let count = u32::from_be_bytes(sensor.read(count_key, info)?.bytes[..4].try_into().ok()?);
        for index in 0..count {
            let Some(entry) = sensor.call(Message {
                command: 8,
                index,
                ..Default::default()
            }) else {
                continue;
            };
            let key = entry.key;
            if gpu_key(key) {
                if let Some(info) = sensor.info(key) {
                    if matches!((info.kind.to_be_bytes(), info.size), (kind, 4) if kind == *b"flt ")
                        || (info.kind == u32::from_be_bytes(*b"sp78") && info.size == 2)
                    {
                        sensor.keys.push((key, info));
                    }
                }
            }
        }
        Some(sensor)
    }

    pub(super) fn hottest(&self) -> Option<f32> {
        self.keys
            .iter()
            .filter_map(|&(key, info)| {
                let value = self.read(key, info)?;
                decode(info, &value.bytes)
            })
            .reduce(f32::max)
    }

    fn info(&self, key: u32) -> Option<KeyInfo> {
        Some(
            self.call(Message {
                key,
                command: 9,
                ..Default::default()
            })?
            .info,
        )
    }

    fn read(&self, key: u32, info: KeyInfo) -> Option<Message> {
        self.call(Message {
            key,
            info,
            command: 5,
            ..Default::default()
        })
    }

    fn call(&self, input: Message) -> Option<Message> {
        let mut output = Message::default();
        let mut size = std::mem::size_of::<Message>();
        // AppleSMC selector 2 uses this 80-byte C layout for read commands.
        let status = unsafe {
            IOConnectCallStructMethod(
                self.connection,
                2,
                (&input as *const Message).cast(),
                size,
                (&mut output as *mut Message).cast(),
                &mut size,
            )
        };
        (status == 0 && size == std::mem::size_of::<Message>() && output.result == 0)
            .then_some(output)
    }
}

fn gpu_key(key: u32) -> bool {
    let bytes = key.to_be_bytes();
    bytes.starts_with(b"Tg") || bytes.starts_with(b"TG")
}

fn decode(info: KeyInfo, bytes: &[u8; 32]) -> Option<f32> {
    let value = match (info.kind.to_be_bytes(), info.size) {
        (kind, 4) if kind == *b"flt " => f32::from_le_bytes(bytes[..4].try_into().ok()?),
        (kind, 2) if kind == *b"sp78" => {
            i16::from_be_bytes(bytes[..2].try_into().ok()?) as f32 / 256.
        }
        _ => return None,
    };
    (value.is_finite() && (0.0..150.0).contains(&value) && value != 0.).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_sensor_formats_and_rejects_missing_readings() {
        assert_eq!(std::mem::size_of::<Message>(), 80);
        assert_eq!(std::mem::offset_of!(Message, info), 28);
        assert_eq!(std::mem::offset_of!(Message, command), 42);
        assert_eq!(std::mem::offset_of!(Message, bytes), 48);
        let mut bytes = [0; 32];
        let float = KeyInfo {
            size: 4,
            kind: u32::from_be_bytes(*b"flt "),
            attributes: 0,
        };
        bytes[..4].copy_from_slice(&52.5f32.to_le_bytes());
        assert_eq!(decode(float, &bytes), Some(52.5));
        for value in [0., f32::NAN, f32::INFINITY, 150., -1.] {
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(decode(float, &bytes), None);
        }
        bytes[..2].copy_from_slice(&13440i16.to_be_bytes());
        assert_eq!(
            decode(
                KeyInfo {
                    size: 2,
                    kind: u32::from_be_bytes(*b"sp78"),
                    attributes: 0
                },
                &bytes
            ),
            Some(52.5)
        );
        assert!(gpu_key(u32::from_be_bytes(*b"Tg0f")));
        assert!(gpu_key(u32::from_be_bytes(*b"TG0D")));
        assert!(!gpu_key(u32::from_be_bytes(*b"Tp01")));
    }
}
