use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Base {
    pub ground: u32,
    pub plate: u32,
    pub silk: u32,
    pub led: u32,
    pub hot: u32,
    pub ok: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub base: Base,
}
pub fn presets() -> Vec<Palette> {
    vec![
        Palette {
            name: "Amber".into(),
            base: Base {
                ground: 0x0b0c0d,
                plate: 0x141618,
                silk: 0xe8e6df,
                led: 0xffb000,
                hot: 0xff3b1f,
                ok: 0x38ff8b,
            },
        },
        Palette {
            name: "Ice".into(),
            base: Base {
                ground: 0x07090c,
                plate: 0x0f141a,
                silk: 0xdfe8f0,
                led: 0x43d6ff,
                hot: 0xff3b6b,
                ok: 0xb4ff39,
            },
        },
        Palette {
            name: "Phosphor".into(),
            base: Base {
                ground: 0x050805,
                plate: 0x0c120d,
                silk: 0xd6f5d9,
                led: 0x39ff6a,
                hot: 0xff4a2e,
                ok: 0xd8ff5a,
            },
        },
        Palette {
            name: "Neon".into(),
            base: Base {
                ground: 0x0a0710,
                plate: 0x130e1c,
                silk: 0xf1e6ff,
                led: 0xff2bd6,
                hot: 0xffe600,
                ok: 0x00f0ff,
            },
        },
        Palette {
            name: "Aluminium".into(),
            base: Base {
                ground: 0xe8e9e5,
                plate: 0xf5f5f0,
                silk: 0x252a2b,
                led: 0x923d18,
                hot: 0xac292a,
                ok: 0x236344,
            },
        },
    ]
}
#[derive(Clone, Copy)]
pub struct Tokens {
    pub base: Base,
    pub plate2: u32,
    pub rule: u32,
    pub rule_hi: u32,
    pub silk_dim: u32,
    pub ink_row: u32,
    pub seg_off: u32,
    pub hot_off: u32,
    pub led_off: u32,
    pub well: u32,
    pub row_rule: u32,
    pub cmd: u32,
    pub on_led: u32,
    pub light: bool,
    pub table: u32,
    pub table_head: u32,
    pub selection: u32,
    pub readout: u32,
    pub well_rule: u32,
}
fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn gamma(v: f64) -> f64 {
    let v = v.clamp(0., 1.);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}
fn lab(c: u32) -> [f64; 3] {
    let r = linear((c >> 16) as f64 / 255.);
    let g = linear(((c >> 8) & 255) as f64 / 255.);
    let b = linear((c & 255) as f64 / 255.);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}
fn from_lab([l, a, b]: [f64; 3]) -> u32 {
    let ll = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (l - 0.0894841775 * a - 1.291485548 * b).powi(3);
    let rgb = [
        4.0767416621 * ll - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * ll + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * ll - 0.7034186147 * m + 1.707614701 * s,
    ];
    rgb.into_iter()
        .fold(0, |n, v| (n << 8) | (gamma(v) * 255.).round() as u32)
}
pub fn mix(a: u32, b: u32, p: f64) -> u32 {
    let a = lab(a);
    let b = lab(b);
    from_lab(std::array::from_fn(|i| a[i] * p + b[i] * (1. - p)))
}
impl Base {
    pub fn tokens(self) -> Tokens {
        let b = self;
        let light = (0.2126 * (b.ground >> 16) as f64
            + 0.7152 * ((b.ground >> 8) & 255) as f64
            + 0.0722 * (b.ground & 255) as f64)
            / 255.
            > 0.4;
        Tokens {
            base: b,
            light,
            plate2: mix(b.plate, b.silk, if light { 0.94 } else { 0.93 }),
            rule: mix(b.plate, b.silk, if light { 0.82 } else { 0.86 }),
            rule_hi: mix(b.plate, b.silk, if light { 0.46 } else { 0.74 }),
            silk_dim: mix(b.silk, b.plate, if light { 0.76 } else { 0.58 }),
            ink_row: mix(b.silk, b.plate, 0.78),
            seg_off: mix(b.plate, b.silk, if light { 0.91 } else { 0.9 }),
            hot_off: mix(b.hot, b.plate, 0.2),
            led_off: mix(b.led, b.plate, 0.16),
            well: mix(
                b.ground,
                if light { 0xffffff } else { 0 },
                if light { 0.8 } else { 0.6 },
            ),
            row_rule: mix(b.ground, b.silk, if light { 0.88 } else { 0.94 }),
            cmd: mix(b.silk, b.ground, if light { 0.76 } else { 0.48 }),
            on_led: if light { 0xffffff } else { 0 },
            table: if light {
                mix(b.ground, 0xffffff, 0.35)
            } else {
                mix(b.ground, 0, 0.8)
            },
            table_head: if light {
                b.ground
            } else {
                mix(b.plate, 0, 0.6)
            },
            selection: if light {
                mix(b.plate, b.led, 0.92)
            } else {
                mix(mix(b.plate, b.silk, 0.93), b.led, 0.7)
            },
            readout: if light {
                mix(b.ground, 0xffffff, 0.35)
            } else {
                mix(b.ground, 0, 0.8)
            },
            well_rule: if light { mix(b.plate, b.silk, 0.82) } else { 0 },
        }
    }
}
pub fn random_base(r: [f64; 5]) -> Base {
    let h = r[0] * 360.;
    let light = r[1] < 0.2;
    let tint = 0.008 + r[2] * 0.03;
    let hot = (h + 130. + r[3] * 100.) % 360.;
    let ok = (h + 60. + r[4] * 60.) % 360.;
    let c = |l, c, h: f64| from_lab([l, c * h.to_radians().cos(), c * h.to_radians().sin()]);
    if light {
        Base {
            ground: c(0.7, tint, h),
            plate: c(0.85, tint, h),
            silk: c(0.17, tint, h),
            led: c(0.64, 0.19, h),
            hot: c(0.5, 0.2, hot),
            ok: c(0.52, 0.14, ok),
        }
    } else {
        Base {
            ground: c(0.12, tint, h),
            plate: c(0.17, tint + 0.004, h),
            silk: c(0.93, 0.02, h),
            led: c(0.8, 0.17, h),
            hot: c(0.67, 0.22, hot),
            ok: c(0.86, 0.17, ok),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub active: usize,
    pub saved: Vec<Palette>,
    #[serde(default, deserialize_with = "read_widths")]
    pub process_widths: crate::column_widths::Widths,
    #[serde(default, deserialize_with = "read_widths")]
    pub history_widths: crate::column_widths::Widths,
}
fn read_widths<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<crate::column_widths::Widths, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

pub async fn next_settings(
    receiver: &mut futures::channel::mpsc::UnboundedReceiver<Settings>,
) -> Option<Settings> {
    use futures::StreamExt;
    let mut latest = receiver.next().await?;
    while let Ok(settings) = receiver.try_recv() {
        latest = settings;
    }
    Some(latest)
}

pub async fn finish_writer<W>(
    sender: futures::channel::mpsc::UnboundedSender<Settings>,
    latest: Settings,
    writer: W,
) where
    W: std::future::Future<Output = ()>,
{
    let _ = sender.unbounded_send(latest);
    drop(sender);
    writer.await;
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(super::history::support_path("palette.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }
    pub fn palettes(&self) -> Vec<Palette> {
        let mut p = presets();
        p.extend(self.saved.clone());
        p
    }
    pub fn base(&self) -> Base {
        self.palettes()
            .get(self.active)
            .unwrap_or(&presets()[0])
            .base
    }
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&super::history::support_path("palette.json"))
    }
    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        static NEXT_WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT_WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_extension(format!("{}.{}.tmp", std::process::id(), sequence));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            std::fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: u32) -> f64 {
        0.2126 * linear((color >> 16) as f64 / 255.)
            + 0.7152 * linear(((color >> 8) & 255) as f64 / 255.)
            + 0.0722 * linear((color & 255) as f64 / 255.)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn aluminium_contrast_covers_text_controls_and_meters() {
        let t = presets()[4].base.tokens();
        let backgrounds = [
            ("ground", t.base.ground),
            ("plate", t.base.plate),
            ("secondary", t.plate2),
            ("well", t.well),
            ("table", t.table),
            ("header", t.table_head),
            ("selection", t.selection),
            ("readout", t.readout),
        ];
        for (name, color) in [
            ("silk", t.base.silk),
            ("dim", t.silk_dim),
            ("row", t.ink_row),
            ("command", t.cmd),
            ("accent", t.base.led),
            ("critical", t.base.hot),
            ("positive", t.base.ok),
        ] {
            for (surface, background) in backgrounds {
                let ratio = contrast(color, background);
                println!("text {name}/{surface}: {ratio:.2}:1");
                assert!(ratio >= 4.5, "{name}/{surface}: {ratio:.2}:1 < 4.5");
            }
        }
        for (name, foreground, background) in [
            ("primary button", t.on_led, t.base.led),
            ("danger button", t.on_led, t.base.hot),
        ] {
            assert!(contrast(foreground, background) >= 4.5, "{name}");
        }
        for (name, surface) in backgrounds {
            assert!(
                contrast(t.rule_hi, surface) >= 3.,
                "control outline/{name}: {}",
                contrast(t.rule_hi, surface)
            );
        }
        for ink in [t.base.led, t.base.hot, t.base.ok] {
            for background in [
                t.readout,
                t.seg_off,
                mix(t.base.led, t.base.ground, 0.12),
                t.hot_off,
            ] {
                assert!(
                    contrast(ink, background) >= 3.,
                    "meter {ink:06x}/{background:06x}: {}",
                    contrast(ink, background)
                );
            }
        }
    }

    #[test]
    fn invalid_width_data_keeps_palette_selection() {
        let settings: Settings = serde_json::from_str(r#"{"active":3,"saved":[],"process_widths":{"columns":{"name":"bad"},"fill_name":true},"history_widths":null}"#).unwrap();
        assert_eq!(settings.active, 3);
        assert_eq!(settings.process_widths, Default::default());
        assert_eq!(settings.history_widths, Default::default());
    }

    #[test]
    fn column_settings_load_old_files_and_round_trip() {
        use crate::column_widths::Column;
        let mut settings: Settings = serde_json::from_str(r#"{"active":0,"saved":[]}"#).unwrap();
        assert_eq!(settings.process_widths, Default::default());
        assert_eq!(settings.history_widths, Default::default());
        settings.process_widths.set(Column::Name, 333.);
        settings.history_widths.set(Column::CoreTime, 177.);
        let directory =
            std::env::temp_dir().join(format!("gtop-column-settings-{}", std::process::id()));
        let path = directory.join("palette.json");
        settings.save_to(&path).unwrap();
        let stored: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.process_widths, settings.process_widths);
        assert_eq!(stored.history_widths, settings.history_widths);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn queued_settings_keep_latest_state_after_a_pending_write() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        sender
            .unbounded_send(Settings {
                active: 0,
                ..Default::default()
            })
            .unwrap();
        let first = futures::executor::block_on(next_settings(&mut receiver)).unwrap();
        for active in 1..100 {
            sender
                .unbounded_send(Settings {
                    active,
                    ..Default::default()
                })
                .unwrap();
        }
        let directory =
            std::env::temp_dir().join(format!("gtop-settings-order-{}", std::process::id()));
        let path = directory.join("palette.json");
        first.save_to(&path).unwrap();
        let latest = futures::executor::block_on(next_settings(&mut receiver)).unwrap();
        latest.save_to(&path).unwrap();
        let stored: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.active, 99);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn finish_writer_publishes_latest_settings_before_returning() {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        sender
            .unbounded_send(Settings {
                active: 1,
                ..Default::default()
            })
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("gtop-settings-shutdown-{}", std::process::id()));
        let path = directory.join("palette.json");
        let writer_path = path.clone();
        let writer = async move {
            while let Some(settings) = next_settings(&mut receiver).await {
                settings.save_to(&writer_path).unwrap();
            }
        };
        futures::executor::block_on(finish_writer(
            sender,
            Settings {
                active: 2,
                ..Default::default()
            },
            writer,
        ));
        let stored: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored.active, 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tokens_preserve_bases_and_choose_light_rules() {
        for p in presets() {
            let t = p.base.tokens();
            assert_eq!(t.base, p.base);
            assert_eq!(t.light, p.name == "Aluminium");
            assert_eq!(t.on_led, if t.light { 0xffffff } else { 0 });
            assert_eq!(mix(p.base.led, p.base.led, 0.5), p.base.led);
        }
    }
    #[test]
    fn random_uses_dark_and_light_ranges() {
        for i in 0..360 {
            for light in [0., 0.9] {
                let b = random_base([i as f64 / 360., light, 0.5, 0.5, 0.5]);
                assert_eq!(b.tokens().light, light < 0.2);
                assert_ne!(b.led, b.hot);
                assert_ne!(b.led, b.ok);
                assert!(lab(b.silk)[0] > 0.9 || lab(b.silk)[0] < 0.2);
            }
        }
    }
}
