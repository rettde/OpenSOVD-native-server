// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// DataCatalogProvider — Pluggable semantic metadata for diagnostic data items
//
// Wave 4, A4.2: Abstracts the source of semantic annotations (unit, range,
// VSS path, data type, sampling hint). Default impl returns static metadata.
// OEM profiles can override via `OemProfile::data_catalog_provider()`.
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// Semantic metadata for a single diagnostic data item.
///
/// Returned by `DataCatalogProvider::metadata()` to enrich `SovdDataCatalogEntry`
/// with machine-readable annotations for ML pipelines.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DataSemantics {
    /// Physical unit (SI or automotive convention), e.g. "V", "°C", "rpm", "bar"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,

    /// Normal operating range as `[min, max]`. ML pipelines use this for
    /// anomaly detection and feature normalization.
    #[serde(skip_serializing_if = "Option::is_none", rename = "normalRange")]
    pub normal_range: Option<NormalRange>,

    /// Semantic reference path (COVESA VSS dot-notation or `x-vendor.*` prefix).
    /// Example: `"Vehicle.Powertrain.Battery.StateOfCharge"`
    #[serde(skip_serializing_if = "Option::is_none", rename = "semanticRef")]
    pub semantic_ref: Option<String>,

    /// Data type hint for downstream schema inference.
    /// Mirrors `SovdDataType` but as a string for portability.
    #[serde(skip_serializing_if = "Option::is_none", rename = "dataType")]
    pub data_type: Option<String>,

    /// Recommended sampling interval in seconds for time-series collection.
    /// `None` means "read on demand" (event-driven data).
    #[serde(skip_serializing_if = "Option::is_none", rename = "samplingHint")]
    pub sampling_hint: Option<f64>,

    /// Classification tags for ML feature engineering (e.g. "powertrain", "safety", "comfort")
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "classificationTags"
    )]
    pub classification_tags: Vec<String>,
}

/// Normal operating range for a data value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalRange {
    pub min: f64,
    pub max: f64,
}

/// Pluggable provider of semantic metadata for diagnostic data items.
///
/// The default implementation (`StaticDataCatalogProvider`) returns empty metadata.
/// OEM-specific implementations can resolve VSS paths from CDF/ODX files,
/// external ontology services, or static lookup tables.
pub trait DataCatalogProvider: Send + Sync {
    /// Return semantic metadata for a specific data item on a component.
    ///
    /// Returns `None` if no metadata is available for this data item.
    fn metadata(&self, component_id: &str, data_id: &str) -> Option<DataSemantics>;

    /// Return semantic metadata for all known data items on a component.
    ///
    /// Default: returns an empty map. Override for bulk enrichment.
    fn all_metadata(&self, _component_id: &str) -> Vec<(String, DataSemantics)> {
        Vec::new()
    }

    /// Schema version string for the semantic metadata contract.
    /// ML pipelines use this to detect breaking changes (E4.1).
    fn schema_version(&self) -> String {
        "1.0.0".to_owned()
    }
}

/// Default no-op provider — returns no semantic metadata.
///
/// Used when no OEM-specific metadata source is configured.
pub struct StaticDataCatalogProvider {
    entries: Vec<StaticEntry>,
}

struct StaticEntry {
    component_id: String,
    data_id: String,
    semantics: DataSemantics,
}

impl StaticDataCatalogProvider {
    /// Create an empty provider (no metadata).
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register semantic metadata for a specific data item.
    #[must_use]
    pub fn add(
        mut self,
        component_id: impl Into<String>,
        data_id: impl Into<String>,
        semantics: DataSemantics,
    ) -> Self {
        self.entries.push(StaticEntry {
            component_id: component_id.into(),
            data_id: data_id.into(),
            semantics,
        });
        self
    }
}

impl Default for StaticDataCatalogProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DataCatalogProvider for StaticDataCatalogProvider {
    fn metadata(&self, component_id: &str, data_id: &str) -> Option<DataSemantics> {
        self.entries
            .iter()
            .find(|e| e.component_id == component_id && e.data_id == data_id)
            .map(|e| e.semantics.clone())
    }

    fn all_metadata(&self, component_id: &str) -> Vec<(String, DataSemantics)> {
        self.entries
            .iter()
            .filter(|e| e.component_id == component_id)
            .map(|e| (e.data_id.clone(), e.semantics.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_provider_returns_none() {
        let provider = StaticDataCatalogProvider::new();
        assert!(provider.metadata("hpc", "battery_voltage").is_none());
        assert!(provider.all_metadata("hpc").is_empty());
    }

    #[test]
    fn static_provider_lookup() {
        let provider = StaticDataCatalogProvider::new().add(
            "hpc",
            "battery_voltage",
            DataSemantics {
                unit: Some("V".into()),
                normal_range: Some(NormalRange {
                    min: 11.5,
                    max: 14.5,
                }),
                semantic_ref: Some("Vehicle.Powertrain.Battery.Voltage".into()),
                data_type: Some("float".into()),
                sampling_hint: Some(1.0),
                classification_tags: vec!["powertrain".into()],
            },
        );

        let meta = provider.metadata("hpc", "battery_voltage").unwrap();
        assert_eq!(meta.unit.as_deref(), Some("V"));
        assert_eq!(
            meta.semantic_ref.as_deref(),
            Some("Vehicle.Powertrain.Battery.Voltage")
        );
        assert_eq!(meta.normal_range.as_ref().unwrap().min, 11.5);
        assert_eq!(meta.sampling_hint, Some(1.0));
        assert_eq!(meta.classification_tags, vec!["powertrain"]);

        // Different component returns None
        assert!(provider.metadata("ecu2", "battery_voltage").is_none());
    }

    #[test]
    fn all_metadata_filters_by_component() {
        let provider = StaticDataCatalogProvider::new()
            .add("hpc", "volt", DataSemantics::default())
            .add("hpc", "temp", DataSemantics::default())
            .add("ecu2", "speed", DataSemantics::default());

        let hpc_meta = provider.all_metadata("hpc");
        assert_eq!(hpc_meta.len(), 2);
        let ecu2_meta = provider.all_metadata("ecu2");
        assert_eq!(ecu2_meta.len(), 1);
        assert!(provider.all_metadata("unknown").is_empty());
    }

    #[test]
    fn schema_version_default() {
        let provider = StaticDataCatalogProvider::new();
        assert_eq!(provider.schema_version(), "1.0.0");
    }

    #[test]
    fn data_semantics_serde_roundtrip() {
        let semantics = DataSemantics {
            unit: Some("°C".into()),
            normal_range: Some(NormalRange {
                min: -40.0,
                max: 120.0,
            }),
            semantic_ref: Some("Vehicle.Powertrain.CoolantTemperature".into()),
            data_type: Some("float".into()),
            sampling_hint: Some(5.0),
            classification_tags: vec!["powertrain".into(), "thermal".into()],
        };
        let json = serde_json::to_string(&semantics).unwrap();
        let parsed: DataSemantics = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, semantics);
    }

    #[test]
    fn data_semantics_empty_skips_none_fields() {
        let semantics = DataSemantics::default();
        let json = serde_json::to_string(&semantics).unwrap();
        // Should be a minimal JSON object with no optional fields
        assert_eq!(json, "{}");
    }
}
