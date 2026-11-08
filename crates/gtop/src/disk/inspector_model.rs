use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct InspectorWidth(f32);

impl InspectorWidth {
    pub(crate) fn new(value: f32) -> Self {
        Self(if value.is_finite() {
            value.clamp(280., 560.)
        } else {
            380.
        })
    }

    pub(crate) fn pixels(self) -> f32 {
        self.0
    }

    pub(crate) fn for_window(self, window: f32) -> Self {
        // Treemap has no tower. Reserve canvas space and outer gaps.
        Self(self.0.min((window - 640.).clamp(280., 560.)))
    }
}

impl Default for InspectorWidth {
    fn default() -> Self {
        Self(380.)
    }
}

impl<'de> Deserialize<'de> for InspectorWidth {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(de)?;
        Ok(Self::new(value.as_f64().map_or(380., |v| v as f32)))
    }
}

#[derive(Default)]
pub(super) struct InspectorResize(Option<(f32, InspectorWidth)>);

impl InspectorResize {
    pub(super) fn begin(&mut self, x: f32, width: InspectorWidth) {
        self.0 = Some((x, width));
    }
    pub(super) fn width_at(&self, x: f32) -> Option<InspectorWidth> {
        self.0
            .map(|(start, width)| InspectorWidth::new(width.pixels() + start - x))
    }
    pub(super) fn active(&self) -> bool {
        self.0.is_some()
    }
    pub(super) fn finish(&mut self) -> bool {
        self.0.take().is_some()
    }
}

pub(super) fn share(bytes: u64, denominator: u64) -> String {
    if denominator == 0 {
        "Unknown".into()
    } else {
        format!("{:.1}%", bytes as f64 / denominator as f64 * 100.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shares_keep_their_denominators() {
        assert_eq!(share(25, 100), "25.0%");
        assert_eq!(share(25, 50), "50.0%");
        assert_eq!(share(25, 0), "Unknown");
        assert_eq!(share(0, 100), "0.0%");
    }
    #[test]
    fn width_clamps_drag_and_reset() {
        assert_eq!(InspectorWidth::new(f32::NAN), InspectorWidth::default());
        assert_eq!(InspectorWidth::new(20.).pixels(), 280.);
        assert_eq!(InspectorWidth::new(900.).pixels(), 560.);
        let width = InspectorWidth::new(560.);
        assert_eq!(width.for_window(900.).pixels(), 280.);
        assert_eq!(width.for_window(1200.).pixels(), 560.);
        let mut drag = InspectorResize::default();
        assert!(!drag.active());
        assert!(drag.width_at(5.).is_none());
        drag.begin(100., InspectorWidth::default());
        assert!(drag.active());
        assert_eq!(drag.width_at(50.).unwrap().pixels(), 430.);
        assert_eq!(drag.width_at(1000.).unwrap().pixels(), 280.);
        assert_eq!(drag.width_at(-1000.).unwrap().pixels(), 560.);
        assert!(drag.finish());
        assert!(!drag.finish());
    }
}
