//! Federated **atproto wire records** — the public, anonymized per-sample summaries
//! Navigator computes at the edge and publishes to a researcher's PDS, and that the
//! AppView's Jetstream consumer mirrors into its `fed.*` reporting tables.
//!
//! This module is the **single source of truth** for those record contracts: the
//! NSID (collection) constants, the field shapes, and the float encoding. Both
//! `navigator-sync` (serialize → publish) and the AppView's `du-jobs` ingest
//! (deserialize → store) depend on it, so the two ends cannot drift.
//!
//! ## No floats on the wire
//!
//! atproto records are DAG-CBOR; the PDS rejects float values. So every `f64`
//! metric rides as a string via [`WireF64`] (lossless shortest round-trip on the
//! way out, parsed from string **or** number on the way back). Genuine integers
//! stay numeric. For the AppView's JSONB columns (which the report UI reads with
//! `as_f64`), each record exposes a numeric *storage-JSON* projection so real
//! numbers land in Postgres while the wire stays float-free.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ── collection NSIDs (canonical; shared by publisher + consumer) ────────────────

/// Coverage / alignment summary.
pub const NS_ALIGNMENT: &str = "com.decodingus.atmosphere.alignment";
/// Anonymized biosample (pseudonymous DID, sex, Y/mt calls, center — no PII).
pub const NS_BIOSAMPLE: &str = "com.decodingus.atmosphere.biosample";
/// Sequencing run characterization (platform/instrument/test — no files).
pub const NS_SEQUENCERUN: &str = "com.decodingus.atmosphere.sequencerun";
/// Ancestry composition (population breakdown).
pub const NS_POPULATION_BREAKDOWN: &str = "com.decodingus.atmosphere.populationBreakdown";
/// Donor-level multi-run haplogroup reconciliation (defined for completeness; its
/// full payload lives with the reconciliation feature).
pub const NS_HAPLOGROUP_RECONCILIATION: &str = "com.decodingus.atmosphere.haplogroupReconciliation";

// ── float-as-string wire scalar ─────────────────────────────────────────────────

/// An `f64` that serializes to a JSON **string** (DAG-CBOR-safe) and deserializes
/// from either a string or a number (tolerant of older/numeric producers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireF64(pub f64);

impl From<f64> for WireF64 {
    fn from(v: f64) -> Self {
        WireF64(v)
    }
}

impl WireF64 {
    /// This value as a JSON **number** — for the AppView's numeric storage columns.
    pub fn as_json_number(self) -> Value {
        json!(self.0)
    }
}

impl Serialize for WireF64 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // `f64::to_string` is the shortest representation that round-trips.
        s.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WireF64 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = WireF64;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number or a numeric string")
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<WireF64, E> {
                Ok(WireF64(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<WireF64, E> {
                Ok(WireF64(v as f64))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<WireF64, E> {
                Ok(WireF64(v as f64))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<WireF64, E> {
                v.parse::<f64>().map(WireF64).map_err(|_| E::custom(format!("invalid numeric string: {v}")))
            }
        }
        d.deserialize_any(V)
    }
}

// ── record envelope ─────────────────────────────────────────────────────────────

/// Version + timestamps the consumer keys ordering/sync on. The AppView reads
/// `meta.createdAt`; `version` lets the contract evolve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordMeta {
    pub version: i64,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl RecordMeta {
    /// A v1 record stamped `created_at` (RFC3339), no update time.
    pub fn v1(created_at: impl Into<String>) -> Self {
        RecordMeta { version: 1, created_at: created_at.into(), updated_at: None }
    }
}

// ── alignment (coverage) ────────────────────────────────────────────────────────

/// The coverage metrics block (nested `metrics{}` the AppView ingest reads).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageMetrics {
    pub mean_coverage: WireF64,
    pub median_coverage: WireF64,
    pub sd_coverage: WireF64,
    pub pct_10x: WireF64,
    pub pct_20x: WireF64,
    pub pct_30x: WireF64,
    pub genome_territory: i64,
    pub callable_bases: i64,
}

/// Per-alignment coverage summary (`com.decodingus.atmosphere.alignment`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentRecord {
    #[serde(rename = "$type")]
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biosample_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_run_ref: Option<String>,
    pub reference_build: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aligner: Option<String>,
    pub metrics: CoverageMetrics,
    pub meta: RecordMeta,
}

impl AlignmentRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        reference_build: impl Into<String>,
        aligner: Option<String>,
        mean_coverage: f64,
        median_coverage: f64,
        sd_coverage: f64,
        pct_10x: f64,
        pct_20x: f64,
        pct_30x: f64,
        genome_territory: u64,
        callable_bases: u64,
        created_at: impl Into<String>,
    ) -> Self {
        AlignmentRecord {
            record_type: NS_ALIGNMENT.to_string(),
            biosample_ref: None,
            sequence_run_ref: None,
            reference_build: reference_build.into(),
            aligner,
            metrics: CoverageMetrics {
                mean_coverage: mean_coverage.into(),
                median_coverage: median_coverage.into(),
                sd_coverage: sd_coverage.into(),
                pct_10x: pct_10x.into(),
                pct_20x: pct_20x.into(),
                pct_30x: pct_30x.into(),
                genome_territory: genome_territory as i64,
                callable_bases: callable_bases as i64,
            },
            meta: RecordMeta::v1(created_at),
        }
    }

    /// Builder: link this record to its biosample and sequence-run records.
    pub fn with_refs(mut self, biosample_ref: Option<String>, sequence_run_ref: Option<String>) -> Self {
        self.biosample_ref = biosample_ref;
        self.sequence_run_ref = sequence_run_ref;
        self
    }
}

// ── biosample (anonymized) ──────────────────────────────────────────────────────

/// One arm's haplogroup call inside [`BiosampleHaplogroups`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaplogroupCall {
    pub haplogroup_name: String,
}

impl HaplogroupCall {
    pub fn new(name: impl Into<String>) -> Self {
        HaplogroupCall { haplogroup_name: name.into() }
    }
}

/// Y / mt haplogroup calls (the AppView reads `haplogroups.{yDna,mtDna}.haplogroupName`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BiosampleHaplogroups {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_dna: Option<HaplogroupCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mt_dna: Option<HaplogroupCall>,
}

/// Anonymized biosample (`com.decodingus.atmosphere.biosample`). PII (donor
/// identifier, accession, free-text description) is **never** carried here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BiosampleRecord {
    #[serde(rename = "$type")]
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sex: Option<String>,
    #[serde(skip_serializing_if = "BiosampleHaplogroups::is_empty", default)]
    pub haplogroups: BiosampleHaplogroups,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population_breakdown_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub str_profile_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence_run_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub genotype_refs: Vec<String>,
    pub meta: RecordMeta,
}

impl BiosampleHaplogroups {
    fn is_empty(&self) -> bool {
        self.y_dna.is_none() && self.mt_dna.is_none()
    }
}

impl BiosampleRecord {
    pub fn new(
        sex: Option<String>,
        y_haplogroup: Option<String>,
        mt_haplogroup: Option<String>,
        center_name: Option<String>,
        created_at: impl Into<String>,
    ) -> Self {
        BiosampleRecord {
            record_type: NS_BIOSAMPLE.to_string(),
            sex,
            haplogroups: BiosampleHaplogroups {
                y_dna: y_haplogroup.map(HaplogroupCall::new),
                mt_dna: mt_haplogroup.map(HaplogroupCall::new),
            },
            center_name,
            population_breakdown_ref: None,
            str_profile_ref: None,
            sequence_run_refs: Vec::new(),
            genotype_refs: Vec::new(),
            meta: RecordMeta::v1(created_at),
        }
    }

    /// Builder: attach join references (sequence runs, ancestry/STR records).
    pub fn with_refs(
        mut self,
        sequence_run_refs: Vec<String>,
        population_breakdown_ref: Option<String>,
        str_profile_ref: Option<String>,
    ) -> Self {
        self.sequence_run_refs = sequence_run_refs;
        self.population_breakdown_ref = population_breakdown_ref;
        self.str_profile_ref = str_profile_ref;
        self
    }
}

// ── sequence run ────────────────────────────────────────────────────────────────

/// Sequencing run characterization (`com.decodingus.atmosphere.sequencerun`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceRunRecord {
    #[serde(rename = "$type")]
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biosample_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_reads: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_length: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_insert_size: Option<WireF64>,
    pub meta: RecordMeta,
}

impl SequenceRunRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        biosample_ref: Option<String>,
        platform_name: Option<String>,
        instrument_model: Option<String>,
        instrument_id: Option<String>,
        test_type: Option<String>,
        library_layout: Option<String>,
        total_reads: Option<i64>,
        read_length: Option<i32>,
        mean_insert_size: Option<f64>,
        created_at: impl Into<String>,
    ) -> Self {
        SequenceRunRecord {
            record_type: NS_SEQUENCERUN.to_string(),
            biosample_ref,
            platform_name,
            instrument_model,
            instrument_id,
            test_type,
            library_layout,
            total_reads,
            read_length,
            mean_insert_size: mean_insert_size.map(WireF64),
            meta: RecordMeta::v1(created_at),
        }
    }
}

// ── population breakdown (ancestry) ──────────────────────────────────────────────

/// One population's share of the estimate. Keys match the AppView render's tolerant
/// extractor (`population` + `percentage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopulationComponent {
    pub population: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population_name: Option<String>,
    /// 0.0–100.0.
    pub percentage: WireF64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
}

impl PopulationComponent {
    /// Numeric storage JSON (percentage as a real number) for the JSONB column.
    fn storage_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("population".into(), json!(self.population));
        if let Some(n) = &self.population_name {
            o.insert("populationName".into(), json!(n));
        }
        o.insert("percentage".into(), self.percentage.as_json_number());
        if let Some(r) = self.rank {
            o.insert("rank".into(), json!(r));
        }
        Value::Object(o)
    }
}

/// A continental (super-population) rollup. Keys match the render's extractor
/// (`superPopulation` + `percentage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuperPopulationSummary {
    pub super_population: String,
    pub percentage: WireF64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub populations: Vec<String>,
}

impl SuperPopulationSummary {
    fn storage_json(&self) -> Value {
        json!({
            "superPopulation": self.super_population,
            "percentage": self.percentage.as_json_number(),
            "populations": self.populations,
        })
    }
}

/// Ancestry composition (`com.decodingus.atmosphere.populationBreakdown`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PopulationBreakdownRecord {
    #[serde(rename = "$type")]
    pub record_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biosample_ref: Option<String>,
    /// e.g. "PCA_PROJECTION_GMM", "AF_LIKELIHOOD".
    pub analysis_method: String,
    /// "aims" | "genome-wide".
    pub panel_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_populations: Option<String>,
    pub snps_analyzed: i64,
    pub snps_with_genotype: i64,
    pub snps_missing: i64,
    pub confidence_level: WireF64,
    #[serde(default)]
    pub components: Vec<PopulationComponent>,
    #[serde(default)]
    pub super_population_summary: Vec<SuperPopulationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pca_coordinates: Option<Vec<WireF64>>,
    pub meta: RecordMeta,
}

impl PopulationBreakdownRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        analysis_method: impl Into<String>,
        panel_type: impl Into<String>,
        reference_populations: Option<String>,
        snps_analyzed: i64,
        snps_with_genotype: i64,
        snps_missing: i64,
        confidence_level: f64,
        components: Vec<PopulationComponent>,
        super_population_summary: Vec<SuperPopulationSummary>,
        pca_coordinates: Option<Vec<f64>>,
        created_at: impl Into<String>,
    ) -> Self {
        PopulationBreakdownRecord {
            record_type: NS_POPULATION_BREAKDOWN.to_string(),
            biosample_ref: None,
            analysis_method: analysis_method.into(),
            panel_type: panel_type.into(),
            reference_populations,
            snps_analyzed,
            snps_with_genotype,
            snps_missing,
            confidence_level: confidence_level.into(),
            components,
            super_population_summary,
            pca_coordinates: pca_coordinates.map(|v| v.into_iter().map(WireF64).collect()),
            meta: RecordMeta::v1(created_at),
        }
    }

    pub fn with_biosample_ref(mut self, biosample_ref: Option<String>) -> Self {
        self.biosample_ref = biosample_ref;
        self
    }

    /// Numeric JSON for the `components` JSONB column (percentages as real numbers).
    pub fn components_storage_json(&self) -> Value {
        Value::Array(self.components.iter().map(PopulationComponent::storage_json).collect())
    }

    /// Numeric JSON for the `super_population_summary` JSONB column.
    pub fn super_population_summary_storage_json(&self) -> Value {
        Value::Array(self.super_population_summary.iter().map(SuperPopulationSummary::storage_json).collect())
    }

    /// Numeric JSON for the `pca_coordinates` JSONB column, if present.
    pub fn pca_coordinates_storage_json(&self) -> Option<Value> {
        self.pca_coordinates
            .as_ref()
            .map(|v| Value::Array(v.iter().map(|w| w.as_json_number()).collect()))
    }

    /// `confidence_level` as a real number for the numeric storage column.
    pub fn confidence_level_number(&self) -> f64 {
        self.confidence_level.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No JSON value anywhere in a serialized record is a float (the atproto constraint).
    fn assert_no_floats(v: &Value) {
        match v {
            Value::Number(n) => assert!(!n.is_f64(), "float on the wire: {n}"),
            Value::Array(a) => a.iter().for_each(assert_no_floats),
            Value::Object(o) => o.values().for_each(assert_no_floats),
            _ => {}
        }
    }

    #[test]
    fn alignment_wire_shape_no_floats_and_roundtrips() {
        let rec = AlignmentRecord::new(
            "GRCh38", Some("bwa-mem2".into()), 34.7, 35.0, 6.1, 99.1, 97.8, 94.2, 3_100_000_000, 2_900_000_000,
            "2026-06-05T00:00:00Z",
        )
        .with_refs(Some("at://x/bs/1".into()), Some("at://x/sr/1".into()));
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(v["$type"], NS_ALIGNMENT);
        assert_eq!(v["referenceBuild"], "GRCh38");
        assert_eq!(v["aligner"], "bwa-mem2");
        assert_eq!(v["metrics"]["meanCoverage"], "34.7"); // string on the wire
        assert_eq!(v["metrics"]["genomeTerritory"], 3_100_000_000i64); // integer stays numeric
        assert_eq!(v["meta"]["createdAt"], "2026-06-05T00:00:00Z");
        assert_no_floats(&v);
        let back: AlignmentRecord = serde_json::from_value(v).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn population_breakdown_wire_and_storage_projection() {
        let rec = PopulationBreakdownRecord::new(
            "PCA_PROJECTION_GMM",
            "genome-wide",
            Some("1000G+SGDP".into()),
            600_000,
            598_000,
            2_000,
            0.97,
            vec![
                PopulationComponent { population: "Steppe".into(), population_name: None, percentage: 49.0.into(), rank: Some(1) },
                PopulationComponent { population: "EEF".into(), population_name: Some("Early European Farmer".into()), percentage: 31.0.into(), rank: Some(2) },
                PopulationComponent { population: "WHG".into(), population_name: Some("Western Hunter-Gatherer".into()), percentage: 20.0.into(), rank: Some(3) },
            ],
            vec![SuperPopulationSummary { super_population: "EUR".into(), percentage: 100.0.into(), populations: vec!["Steppe".into()] }],
            Some(vec![0.012, -0.044]),
            "2026-06-05T00:00:00Z",
        )
        .with_biosample_ref(Some("at://x/bs/1".into()));

        // Wire: float-free.
        let wire = serde_json::to_value(&rec).unwrap();
        assert_eq!(wire["$type"], NS_POPULATION_BREAKDOWN);
        assert_eq!(wire["analysisMethod"], "PCA_PROJECTION_GMM");
        assert_eq!(wire["confidenceLevel"], "0.97");
        assert_eq!(wire["components"][0]["population"], "Steppe");
        assert_eq!(wire["components"][0]["percentage"], "49"); // string
        assert_eq!(wire["pcaCoordinates"][0], "0.012");
        assert_no_floats(&wire);

        // Round-trips (string → WireF64).
        let back: PopulationBreakdownRecord = serde_json::from_value(wire).unwrap();
        assert_eq!(back, rec);

        // Storage projection: real numbers for the AppView's JSONB columns + render.
        let comps = rec.components_storage_json();
        assert_eq!(comps[0]["percentage"], 49.0);
        assert_eq!(comps[1]["populationName"], "Early European Farmer");
        assert_eq!(rec.super_population_summary_storage_json()[0]["percentage"], 100.0);
        assert_eq!(rec.pca_coordinates_storage_json().unwrap()[1], -0.044);
        assert_eq!(rec.confidence_level_number(), 0.97);
    }

    #[test]
    fn wiref64_accepts_number_or_string() {
        #[derive(Deserialize)]
        struct H {
            x: WireF64,
        }
        let from_str: H = serde_json::from_str(r#"{"x":"12.5"}"#).unwrap();
        let from_num: H = serde_json::from_str(r#"{"x":12.5}"#).unwrap();
        assert_eq!(from_str.x, WireF64(12.5));
        assert_eq!(from_num.x, WireF64(12.5));
    }

    #[test]
    fn biosample_drops_pii_and_nests_haplogroups() {
        let rec = BiosampleRecord::new(
            Some("Male".into()),
            Some("R-M269".into()),
            Some("U5a1".into()),
            Some("Acme Lab".into()),
            "2026-06-05T00:00:00Z",
        )
        .with_refs(vec!["at://x/sr/1".into()], Some("at://x/pb/1".into()), None);
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(v["$type"], NS_BIOSAMPLE);
        assert_eq!(v["haplogroups"]["yDna"]["haplogroupName"], "R-M269");
        assert_eq!(v["haplogroups"]["mtDna"]["haplogroupName"], "U5a1");
        assert_eq!(v["sequenceRunRefs"][0], "at://x/sr/1");
        assert!(v.get("accession").is_none() && v.get("description").is_none());
        let back: BiosampleRecord = serde_json::from_value(v).unwrap();
        assert_eq!(back, rec);
    }
}
