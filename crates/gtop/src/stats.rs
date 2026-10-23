use crate::{
    model::{bytes, duration},
    theme::*,
};
use gpui::{prelude::*, *};
use gtop_core::SystemStats;

fn panel(title: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(MANTLE))
        .rounded_md()
        .flex_1()
        .min_w(px(140.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
}

fn pair(label: &str, value: String) -> Div {
    div()
        .flex()
        .justify_between()
        .gap_3()
        .text_size(px(12.))
        .child(div().text_color(rgb(SUBTEXT)).child(label.to_string()))
        .child(value)
}

fn bar(value: f32, color: u32) -> Div {
    div()
        .h(px(5.))
        .w_full()
        .rounded_sm()
        .bg(rgb(SURFACE))
        .child(
            div()
                .h_full()
                .w(relative(value.clamp(0., 100.) / 100.))
                .rounded_sm()
                .bg(rgb(color)),
        )
}

pub fn render(stats: &SystemStats) -> Div {
    let average = stats.cpu_usage.iter().sum::<f32>() / stats.cpu_usage.len().max(1) as f32;
    let cpu_columns = stats
        .cpu_usage
        .chunks(stats.cpu_usage.len().div_ceil(2).max(1));
    let cpu =
        panel(&format!("CPU   {average:.1}%"))
            .min_w(px(285.))
            .flex_grow(2.)
            .child(
                div()
                    .flex()
                    .gap_4()
                    .children(cpu_columns.enumerate().map(|(group, cores)| {
                        div().flex().flex_col().gap_1().flex_1().children(
                            cores.iter().enumerate().map(|(index, value)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .w(px(25.))
                                            .text_size(px(11.))
                                            .text_color(rgb(SUBTEXT))
                                            .child(format!(
                                                "{:02}",
                                                group * stats.cpu_usage.len().div_ceil(2) + index
                                            )),
                                    )
                                    .child(div().flex_1().child(bar(
                                        *value,
                                        if *value > 70. { PEACH } else { BLUE },
                                    )))
                                    .child(
                                        div()
                                            .w(px(34.))
                                            .text_size(px(11.))
                                            .text_right()
                                            .child(format!("{value:.0}%")),
                                    )
                            }),
                        )
                    })),
            );
    let gpu = panel("GPU")
        .min_w(px(180.))
        .children(if stats.gpus.is_empty() {
            vec![div()
                .text_size(px(12.))
                .text_color(rgb(SUBTEXT))
                .child("No supported GPU")
                .into_any_element()]
        } else {
            stats
                .gpus
                .iter()
                .map(|gpu| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(12.)).truncate().child(gpu.name.clone()))
                        .child(pair(
                            "Utilization",
                            gpu.utilization
                                .map(|v| format!("{v:.1}%"))
                                .unwrap_or_else(|| "Unavailable".into()),
                        ))
                        .when_some(gpu.utilization, |this, value| this.child(bar(value, TEAL)))
                        .child(pair(
                            "Memory used",
                            gpu.memory_used
                                .map(bytes)
                                .unwrap_or_else(|| "Unavailable".into()),
                        ))
                        .when_some(gpu.memory_total, |this, value| {
                            this.child(pair("Memory total", bytes(value)))
                        })
                        .into_any_element()
                })
                .collect()
        });
    let memory_percent = stats.memory_used as f32 / stats.memory_total.max(1) as f32 * 100.;
    let memory = panel("Memory")
        .child(pair("In use", format!("{memory_percent:.1}%")))
        .child(bar(memory_percent, PEACH))
        .child(pair("Total", bytes(stats.memory_total)))
        .child(pair("Used", bytes(stats.memory_used)))
        .child(pair("Free", bytes(stats.memory_free)));
    let storage_percent =
        stats.disk_used_bytes as f32 / stats.disk_total_bytes.max(1) as f32 * 100.;
    let storage = panel("Storage")
        .child(pair("In use", format!("{storage_percent:.1}%")))
        .child(bar(storage_percent, BLUE))
        .child(pair("Total", bytes(stats.disk_total_bytes)))
        .child(pair("Used", bytes(stats.disk_used_bytes)))
        .child(pair("Free", bytes(stats.disk_free_bytes)));
    let system = panel("System")
        .min_w(px(170.))
        .child(pair("Uptime", duration(stats.uptime)))
        .children(
            stats
                .load_avg
                .iter()
                .zip(["1m load", "5m load", "15m load"])
                .map(|(value, label)| pair(label, format!("{value:.2}"))),
        );
    let network = panel("Network I/O")
        .min_w(px(155.))
        .child(pair(
            "Receiving",
            format!("{}/s", bytes(stats.network_rx_bytes)),
        ))
        .child(pair(
            "Sending",
            format!("{}/s", bytes(stats.network_tx_bytes)),
        ));
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_2()
        .p_3()
        .child(
            div()
                .id("stats")
                .flex()
                .gap_2()
                .overflow_x_scroll()
                .child(cpu)
                .child(gpu)
                .child(memory)
                .child(storage)
                .child(system)
                .child(network),
        )
        .child(
            div()
                .flex()
                .gap_4()
                .text_size(px(12.))
                .text_color(rgb(SUBTEXT))
                .child("Temperatures")
                .when(stats.temperatures.is_empty(), |this| {
                    this.child("No readable sensors")
                })
                .children(stats.temperatures.iter().map(|reading| {
                    div()
                        .text_color(rgb(TEAL))
                        .child(format!("{}  {:.1} °C", reading.name, reading.celsius))
                })),
        )
}
