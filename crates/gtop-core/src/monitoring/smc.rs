//! Read-only AppleSMC GPU temperature and fan speed discovery. Key names vary
//! by chip.

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

impl Message {
    fn info_request(key: u32) -> Self {
        Self {
            key,
            command: 9,
            ..Default::default()
        }
    }

    fn read_request(key: u32, info: KeyInfo) -> Self {
        Self {
            key,
            info,
            command: 5,
            ..Default::default()
        }
    }
}

/// An SMC key and its type and size
type SmcKey = (u32, KeyInfo);

/// SMC keys of one fan: its current speed and, when the SMC has it, its top speed
#[derive(Debug)]
struct FanKeys {
    index: u8,
    actual: SmcKey,
    max: Option<SmcKey>,
}

#[derive(Debug)]
pub(super) struct GpuTemperature {
    connection: u32,
    keys: Vec<(u32, KeyInfo)>,
    fans: Vec<FanKeys>,
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
            fans: Vec::new(),
        };
        let mut fans: [(Option<SmcKey>, Option<SmcKey>); 10] = [(None, None); 10];
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
            if let Some((index, is_max)) = fan_key(key) {
                if let Some(info) = sensor.info(key).filter(|&info| rpm_format(info)) {
                    let slot = &mut fans[index];
                    if is_max {
                        slot.1 = Some((key, info));
                    } else {
                        slot.0 = Some((key, info));
                    }
                }
            } else if gpu_key(key) {
                if let Some(info) = sensor.info(key) {
                    if matches!((info.kind.to_be_bytes(), info.size), (kind, 4) if kind == *b"flt ")
                        || (info.kind == u32::from_be_bytes(*b"sp78") && info.size == 2)
                    {
                        sensor.keys.push((key, info));
                    }
                }
            }
        }
        sensor.fans = (0u8..)
            .zip(fans)
            .filter_map(|(index, (actual, max))| {
                Some(FanKeys {
                    index,
                    actual: actual?,
                    max,
                })
            })
            .collect();
        Some(sensor)
    }

    /// Current speed of every fan, in fan index order; empty on fanless Macs
    pub(super) fn fans(&self) -> Vec<super::Fan> {
        self.fans
            .iter()
            .filter_map(|fan| {
                let (key, info) = fan.actual;
                Some(super::Fan {
                    index: fan.index,
                    rpm: decode_rpm(info, &self.read(key, info)?.bytes)?,
                    max_rpm: fan.max.and_then(|(key, info)| {
                        decode_rpm(info, &self.read(key, info)?.bytes).filter(|&rpm| rpm > 0.)
                    }),
                })
            })
            .collect()
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
        Some(self.call(Message::info_request(key))?.info)
    }

    fn read(&self, key: u32, info: KeyInfo) -> Option<Message> {
        self.call(Message::read_request(key, info))
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

/// Fan keys are `F<digit>Ac` for the current speed and `F<digit>Mx` for the
/// top speed. Returns the fan index and whether the key is the top speed.
fn fan_key(key: u32) -> Option<(usize, bool)> {
    let [f, digit, a, b] = key.to_be_bytes();
    if f != b'F' || !digit.is_ascii_digit() {
        return None;
    }
    let index = usize::from(digit - b'0');
    match [a, b] {
        [b'A', b'c'] => Some((index, false)),
        [b'M', b'x'] => Some((index, true)),
        _ => None,
    }
}

/// Apple silicon stores fan speeds as `flt `, Intel Macs as `fpe2`
fn rpm_format(info: KeyInfo) -> bool {
    matches!(
        (&info.kind.to_be_bytes(), info.size),
        (b"flt ", 4) | (b"fpe2", 2)
    )
}

fn decode_rpm(info: KeyInfo, bytes: &[u8; 32]) -> Option<f32> {
    let value = match (&info.kind.to_be_bytes(), info.size) {
        (b"flt ", 4) => f32::from_le_bytes(bytes[..4].try_into().ok()?),
        (b"fpe2", 2) => f32::from(u16::from_be_bytes(bytes[..2].try_into().ok()?)) / 4.,
        _ => return None,
    };
    (value.is_finite() && (0.0..20_000.0).contains(&value)).then_some(value)
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
        bytes[..4].copy_from_slice(&52.5f32.to_le_bytes());
        let wrong_float = KeyInfo {
            kind: u32::from_be_bytes(*b"sp78"),
            ..float
        };
        assert_eq!(decode(wrong_float, &bytes), None);
        let wrong_fixed = KeyInfo { size: 2, ..float };
        bytes[..2].copy_from_slice(&13440i16.to_be_bytes());
        assert_eq!(decode(wrong_fixed, &bytes), None);
        assert!(gpu_key(u32::from_be_bytes(*b"Tg0f")));
        assert!(gpu_key(u32::from_be_bytes(*b"TG0D")));
        assert!(!gpu_key(u32::from_be_bytes(*b"Tp01")));
    }

    #[test]
    fn finds_fan_keys_and_decodes_fan_speeds() {
        let key = |name: &[u8; 4]| u32::from_be_bytes(*name);
        assert_eq!(fan_key(key(b"F0Ac")), Some((0, false)));
        assert_eq!(fan_key(key(b"F1Mx")), Some((1, true)));
        for other in [b"F0Mn", b"FNum", b"Tg0f", b"FxAc"] {
            assert_eq!(fan_key(key(other)), None);
        }
        let float = KeyInfo {
            size: 4,
            kind: key(b"flt "),
            attributes: 0,
        };
        let fixed = KeyInfo {
            size: 2,
            kind: key(b"fpe2"),
            attributes: 0,
        };
        assert!(rpm_format(float) && rpm_format(fixed));
        assert!(!rpm_format(KeyInfo { size: 2, ..float }));
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&2400f32.to_le_bytes());
        assert_eq!(decode_rpm(float, &bytes), Some(2400.));
        bytes[..4].copy_from_slice(&0f32.to_le_bytes());
        assert_eq!(decode_rpm(float, &bytes), Some(0.));
        for value in [f32::NAN, -1., 20_000.] {
            bytes[..4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(decode_rpm(float, &bytes), None);
        }
        bytes[..2].copy_from_slice(&(1800u16 * 4).to_be_bytes());
        assert_eq!(decode_rpm(fixed, &bytes), Some(1800.));
        assert_eq!(decode_rpm(KeyInfo { size: 4, ..fixed }, &bytes), None);
    }

    #[test]
    fn requests_name_the_key_and_the_smc_command() {
        let key = u32::from_be_bytes(*b"Tg0f");
        let info = Message::info_request(key);
        assert_eq!((info.key, info.command), (key, 9));
        let sensor = KeyInfo {
            size: 4,
            kind: u32::from_be_bytes(*b"flt "),
            attributes: 0,
        };
        let read = Message::read_request(key, sensor);
        assert_eq!((read.key, read.command), (key, 5));
        assert_eq!((read.info.size, read.info.kind), (sensor.size, sensor.kind));
    }
}
