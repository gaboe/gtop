use super::{BatteryState, Power};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RawBattery {
    pub capacity_percent: Option<i64>,
    pub is_charging: Option<bool>,
    pub external_connected: Option<bool>,
    pub fully_charged: Option<bool>,
    pub avg_time_to_full: Option<i64>,
    pub avg_time_to_empty: Option<i64>,
    pub voltage_mv: Option<i64>,
    pub amperage_ma: Option<i64>,
    pub adapter_watts: Option<i64>,
    pub system_load_mw: Option<i64>,
}

pub(super) fn power(raw: &RawBattery) -> Option<Power> {
    let percent = u8::try_from(raw.capacity_percent?)
        .ok()
        .filter(|value| *value <= 100)?;
    let minutes = |value: Option<i64>| {
        value
            .filter(|minutes| (0..65_535).contains(minutes))
            .and_then(|minutes| u32::try_from(minutes).ok())
    };
    let state = match (raw.external_connected, raw.fully_charged, raw.is_charging) {
        (Some(true), Some(true), _) => BatteryState::Full,
        (_, _, Some(true)) => BatteryState::Charging {
            minutes_to_full: minutes(raw.avg_time_to_full),
        },
        (Some(false), _, _) => BatteryState::Discharging {
            minutes_to_empty: minutes(raw.avg_time_to_empty),
        },
        (Some(true), _, _) => BatteryState::NotCharging,
        _ => return None,
    };
    let battery_watts = raw
        .voltage_mv
        .zip(raw.amperage_ma)
        .map(|(voltage, amperage)| (voltage as f64 * amperage as f64 / 1_000_000.) as f32);
    let system_watts = raw
        .system_load_mw
        .filter(|watts| *watts > 0)
        .map(|watts| watts as f32 / 1_000.)
        .or_else(|| {
            if !matches!(&state, BatteryState::Discharging { .. }) {
                return None;
            }
            raw.voltage_mv
                .zip(raw.amperage_ma)
                .map(|(voltage, amperage)| {
                    (voltage as f64 * amperage.unsigned_abs() as f64 / 1_000_000.) as f32
                })
        });

    Some(Power {
        percent,
        state,
        system_watts,
        battery_watts,
        adapter_watts: raw
            .adapter_watts
            .filter(|watts| *watts > 0)
            .and_then(|watts| u32::try_from(watts).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawBattery {
        RawBattery {
            capacity_percent: Some(80),
            is_charging: Some(true),
            external_connected: Some(true),
            fully_charged: Some(false),
            avg_time_to_full: Some(85),
            avg_time_to_empty: Some(240),
            voltage_mv: Some(12_000),
            amperage_ma: Some(1_000),
            adapter_watts: Some(90),
            system_load_mw: Some(70_411),
        }
    }

    #[test]
    fn parses_charging_power_and_adapter_readings() {
        assert_eq!(
            power(&raw()),
            Some(Power {
                percent: 80,
                state: BatteryState::Charging {
                    minutes_to_full: Some(85),
                },
                system_watts: Some(70.411),
                battery_watts: Some(12.),
                adapter_watts: Some(90),
            })
        );
    }

    #[test]
    fn parses_discharging_power_with_signed_battery_watts() {
        let raw = RawBattery {
            is_charging: Some(false),
            external_connected: Some(false),
            system_load_mw: None,
            amperage_ma: Some(-800),
            ..raw()
        };
        assert_eq!(
            power(&raw),
            Some(Power {
                percent: 80,
                state: BatteryState::Discharging {
                    minutes_to_empty: Some(240),
                },
                system_watts: Some(9.6),
                battery_watts: Some(-9.6),
                adapter_watts: Some(90),
            })
        );
    }

    #[test]
    fn distinguishes_full_from_external_power_without_charging() {
        let mut raw = raw();
        raw.is_charging = Some(false);
        raw.fully_charged = Some(true);
        assert_eq!(power(&raw).unwrap().state, BatteryState::Full);

        raw.fully_charged = Some(false);
        assert_eq!(power(&raw).unwrap().state, BatteryState::NotCharging);
    }

    #[test]
    fn ignores_unknown_and_missing_times_and_optional_readings() {
        let raw = RawBattery {
            avg_time_to_full: Some(65_535),
            avg_time_to_empty: None,
            voltage_mv: None,
            amperage_ma: None,
            adapter_watts: None,
            system_load_mw: Some(0),
            ..raw()
        };
        assert_eq!(
            power(&raw),
            Some(Power {
                percent: 80,
                state: BatteryState::Charging {
                    minutes_to_full: None,
                },
                system_watts: None,
                battery_watts: None,
                adapter_watts: None,
            })
        );
    }

    #[test]
    fn prefers_system_load_over_battery_fallback() {
        let raw = RawBattery {
            is_charging: Some(false),
            external_connected: Some(false),
            system_load_mw: Some(70_411),
            amperage_ma: Some(-800),
            ..raw()
        };
        assert_eq!(power(&raw).unwrap().system_watts, Some(70.411));
    }

    #[test]
    fn missing_battery_and_required_state_or_charge_return_none() {
        assert_eq!(power(&RawBattery::default()), None);
        assert_eq!(
            power(&RawBattery {
                capacity_percent: Some(80),
                ..RawBattery::default()
            }),
            None
        );
        assert_eq!(
            power(&RawBattery {
                external_connected: Some(true),
                ..RawBattery::default()
            }),
            None
        );
    }
}
