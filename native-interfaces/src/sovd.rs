// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SOVD API types — aligned with sovd-interfaces (cda-sovd-interfaces)
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// SOVD collection wrapper (OData-conformant, SOVD §5)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection<T: Serialize> {
    #[serde(rename = "@odata.context", skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub value: Vec<T>,
    #[serde(rename = "@odata.count")]
    pub count: usize,
}

impl<T: Serialize> Collection<T> {
    #[must_use]
    pub fn new(items: Vec<T>) -> Self {
        let count = items.len();
        Self {
            context: None,
            value: items,
            count,
        }
    }

    #[must_use]
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }
}

/// SOVD component representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdComponent {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "connectionState")]
    pub connection_state: SovdConnectionState,
    /// Software version installed on this component (Wave 3, W3.3 variant-aware discovery)
    #[serde(skip_serializing_if = "Option::is_none", rename = "softwareVersion")]
    pub software_version: Option<String>,
    /// Hardware variant identifier (e.g. "EU-LHD", "US-RHD")
    #[serde(skip_serializing_if = "Option::is_none", rename = "hardwareVariant")]
    pub hardware_variant: Option<String>,
    /// Installation variant (e.g. "base", "premium", "sport")
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "installationVariant"
    )]
    pub installation_variant: Option<String>,
}

/// SOVD connection state (SOVD §7.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdConnectionState {
    Connected,
    Disconnected,
    Connecting,
    Error,
}

/// SOVD fault representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdFault {
    pub id: String,
    #[serde(rename = "componentId")]
    pub component_id: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "displayCode")]
    pub display_code: Option<String>,
    pub severity: SovdFaultSeverity,
    pub status: SovdFaultStatus,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Fault scope (MBDS §7.1): e.g. "component", "system", "network"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Affected subsystem for ML fault correlation (Wave 4, W4.3)
    #[serde(skip_serializing_if = "Option::is_none", rename = "affectedSubsystem")]
    pub affected_subsystem: Option<String>,
    /// VSS paths of signals correlated with this fault (Wave 4, W4.3)
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "correlatedSignals"
    )]
    pub correlated_signals: Vec<String>,
    /// ML classification tags for feature engineering (Wave 4, W4.3)
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "classificationTags"
    )]
    pub classification_tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SovdFaultSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SovdFaultStatus {
    Active,
    Passive,
    Pending,
}

/// SOVD data resource (SOVD §7.5)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdData {
    pub id: String,
    #[serde(rename = "componentId")]
    pub component_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub access: SovdDataAccess,
    #[serde(rename = "dataType")]
    pub data_type: SovdDataType,
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// SOVD data type classification (SOVD §7.5)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdDataType {
    String,
    Integer,
    Float,
    Boolean,
    Bytes,
    Enum,
    Struct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdDataAccess {
    ReadOnly,
    ReadWrite,
    WriteOnly,
}

/// SOVD operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdOperation {
    pub id: String,
    #[serde(rename = "componentId")]
    pub component_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: SovdOperationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SovdOperationStatus {
    Idle,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// SOVD operation execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdOperationExecution {
    #[serde(rename = "executionId")]
    pub execution_id: String,
    #[serde(rename = "componentId")]
    pub component_id: String,
    #[serde(rename = "operationId")]
    pub operation_id: String,
    pub status: SovdOperationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// OData-conformant error envelope (SOVD §5.4, OData §9.4).
///
/// All SOVD error responses MUST be wrapped: `{"error": { ... }}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdErrorEnvelope {
    pub error: SovdErrorResponse,
}

impl SovdErrorEnvelope {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: SovdErrorResponse {
                code: code.into(),
                message: message.into(),
                target: None,
                details: vec![],
                innererror: None,
            },
        }
    }
}

/// SOVD error response body (SOVD §5.4, OData error format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdErrorResponse {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub details: Vec<SovdErrorDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub innererror: Option<String>,
}

/// Structured error detail entry (OData §9.4)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

// ── Error Code Catalog (SOVD §5.4 + ISO 17978-3) ─────────────────────────

/// Typed SOVD error codes for machine-readable error classification.
///
/// Each variant maps to an HTTP status, a stable string code (e.g. `"SOVD-ERR-404"`),
/// and a default description. Handlers may override the description with context-
/// specific messages while preserving the code for monitoring and automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SovdErrorCode {
    /// 400 — Malformed request, missing/invalid parameters
    BadRequest,
    /// 401 — Missing or invalid authentication credentials
    Unauthorized,
    /// 403 — Authenticated but insufficient permissions
    Forbidden,
    /// 404 — Resource not found
    NotFound,
    /// 409 — Conflicting state (e.g. lock held by another client)
    Conflict,
    /// 422 — Request is well-formed but semantically invalid
    UnprocessableEntity,
    /// 429 — Too many requests (rate-limited)
    TooManyRequests,
    /// 500 — Internal server error
    InternalError,
    /// 501 — Operation not implemented by backend
    NotImplemented,
    /// 502 — ECU offline or CDA unreachable
    BadGateway,
    /// 503 — Server not ready (readiness probe fail)
    ServiceUnavailable,
    /// 504 — Backend or ECU response timeout
    GatewayTimeout,
}

impl SovdErrorCode {
    /// Stable string code for serialization and monitoring.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest => "SOVD-ERR-400",
            Self::Unauthorized => "SOVD-ERR-401",
            Self::Forbidden => "SOVD-ERR-403",
            Self::NotFound => "SOVD-ERR-404",
            Self::Conflict => "SOVD-ERR-409",
            Self::UnprocessableEntity => "SOVD-ERR-422",
            Self::TooManyRequests => "SOVD-ERR-429",
            Self::InternalError => "SOVD-ERR-500",
            Self::NotImplemented => "SOVD-ERR-501",
            Self::BadGateway => "SOVD-ERR-502",
            Self::ServiceUnavailable => "SOVD-ERR-503",
            Self::GatewayTimeout => "SOVD-ERR-504",
        }
    }

    /// Default human-readable description.
    #[must_use]
    pub fn default_message(&self) -> &'static str {
        match self {
            Self::BadRequest => "The request is malformed or contains invalid parameters",
            Self::Unauthorized => "Authentication credentials are missing or invalid",
            Self::Forbidden => "Insufficient permissions for this operation",
            Self::NotFound => "The requested resource was not found",
            Self::Conflict => "The request conflicts with the current resource state",
            Self::UnprocessableEntity => "The request is well-formed but semantically invalid",
            Self::TooManyRequests => "Too many requests — retry after backoff",
            Self::InternalError => "An internal server error occurred",
            Self::NotImplemented => "This operation is not implemented by the backend",
            Self::BadGateway => "The upstream ECU or CDA is unreachable",
            Self::ServiceUnavailable => "The server is not ready to handle requests",
            Self::GatewayTimeout => "The upstream ECU or CDA did not respond in time",
        }
    }

    /// Corresponding HTTP status code.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::UnprocessableEntity => 422,
            Self::TooManyRequests => 429,
            Self::InternalError => 500,
            Self::NotImplemented => 501,
            Self::BadGateway => 502,
            Self::ServiceUnavailable => 503,
            Self::GatewayTimeout => 504,
        }
    }

    /// Convenience: build a `SovdErrorEnvelope` from this code with a custom message.
    #[must_use]
    pub fn envelope(&self, message: impl Into<String>) -> SovdErrorEnvelope {
        SovdErrorEnvelope::new(self.code(), message)
    }

    /// Convenience: build a `SovdErrorEnvelope` with the default message.
    #[must_use]
    pub fn default_envelope(&self) -> SovdErrorEnvelope {
        SovdErrorEnvelope::new(self.code(), self.default_message())
    }
}

impl std::fmt::Display for SovdErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}

// ── Locking (SOVD Standard §7.4) ─────────────────────────────────────────

/// SOVD resource lock
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdLock {
    #[serde(rename = "componentId")]
    pub component_id: String,
    #[serde(rename = "lockedBy")]
    pub locked_by: String,
    #[serde(rename = "lockedAt")]
    pub locked_at: String,
    /// Lock expiration (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

// ── Capabilities (SOVD Standard §7.3) ────────────────────────────────────

/// SOVD component capabilities — describes what a component supports
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdCapabilities {
    #[serde(rename = "componentId")]
    pub component_id: String,
    #[serde(rename = "supportedCategories")]
    pub supported_categories: Vec<String>,
    /// Number of available data identifiers
    #[serde(rename = "dataCount")]
    pub data_count: usize,
    /// Number of available operations
    #[serde(rename = "operationCount")]
    pub operation_count: usize,
    /// Supported features
    pub features: Vec<String>,
}

// ── Data Catalog (SOVD Standard §7.5) ────────────────────────────────────

/// SOVD data catalog entry — metadata about a single DID (SOVD §7.5)
///
/// Wave 4 (W4.1) extends this with semantic metadata for ML pipeline consumption:
/// `normalRange`, `semanticRef`, `samplingHint`, `classificationTags`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdDataCatalogEntry {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub access: SovdDataAccess,
    #[serde(rename = "dataType")]
    pub data_type: SovdDataType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Raw UDS DID (hex) — vendor extension
    #[serde(skip_serializing_if = "Option::is_none", rename = "x-uds-did")]
    pub did: Option<String>,
    /// Normal operating range `[min, max]` for anomaly detection (Wave 4, W4.1)
    #[serde(skip_serializing_if = "Option::is_none", rename = "normalRange")]
    pub normal_range: Option<crate::data_catalog::NormalRange>,
    /// COVESA VSS path or `x-vendor.*` semantic reference (Wave 4, W4.1 / A4.1)
    #[serde(skip_serializing_if = "Option::is_none", rename = "semanticRef")]
    pub semantic_ref: Option<String>,
    /// Recommended sampling interval in seconds (Wave 4, W4.1)
    #[serde(skip_serializing_if = "Option::is_none", rename = "samplingHint")]
    pub sampling_hint: Option<f64>,
    /// ML classification tags (e.g. "powertrain", "safety") (Wave 4, W4.1)
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "classificationTags"
    )]
    pub classification_tags: Vec<String>,
}

// ── Groups (SOVD Standard §7.2) ──────────────────────────────────────────

/// SOVD component group
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdGroup {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "componentIds")]
    pub component_ids: Vec<String>,
}

// ── Proximity Challenge (SOVD Standard §7.9) ─────────────────────────────

/// SOVD proximity challenge request/response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdProximityChallenge {
    #[serde(rename = "challengeId")]
    pub challenge_id: String,
    pub status: SovdProximityChallengeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SovdProximityChallengeStatus {
    Pending,
    Verified,
    Failed,
}

// ── Logs (SOVD Standard §7.10) ───────────────────────────────────────────

/// SOVD diagnostic log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdLogEntry {
    pub timestamp: String,
    pub level: SovdLogLevel,
    pub source: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SovdLogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

// ── Mode / Session (SOVD Standard §7.6) ──────────────────────────────────

/// SOVD diagnostic mode/session (W2.4 enriched model)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdMode {
    #[serde(rename = "componentId")]
    pub component_id: String,
    #[serde(rename = "currentMode")]
    pub current_mode: String,
    #[serde(rename = "availableModes")]
    pub available_modes: Vec<String>,
    /// Detailed mode descriptors (W2.4) — optional for backward compat
    #[serde(
        rename = "modeDescriptors",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub mode_descriptors: Vec<SovdModeDescriptor>,
    /// ISO 8601 timestamp when the current mode was activated
    #[serde(
        rename = "activeSince",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_since: Option<String>,
}

/// Describes a single SOVD mode with its UDS session mapping (W2.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdModeDescriptor {
    /// Mode identifier (e.g. "default", "extended", "programming")
    pub id: String,
    /// Human-readable display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Description of what this mode enables
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// UDS session type this mode maps to (e.g. 0x01 = default, 0x02 = programming, 0x03 = extended)
    #[serde(
        rename = "udsSession",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub uds_session: Option<u8>,
    /// Whether this mode requires security access (UDS 0x27)
    #[serde(rename = "requiresSecurityAccess", default)]
    pub requires_security_access: bool,
}

// ── Configuration (SOVD Standard §7.8) ───────────────────────────────────

/// SOVD component configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdComponentConfig {
    #[serde(rename = "componentId")]
    pub component_id: String,
    pub parameters: serde_json::Value,
}

// ── Apps (ISO 17978-3 §4.2.3) ─────────────────────────────────────────────

/// SOVD application entity — a diagnostic application hosted on the HPC
/// (e.g. "SOTA Manager", "Flash Master", "Health Monitor").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdApp {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub version: String,
    pub status: SovdAppStatus,
}

/// Application lifecycle status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdAppStatus {
    Running,
    Stopped,
    Error,
}

// ── Funcs (ISO 17978-3 §4.2.3) ────────────────────────────────────────────

/// SOVD function entity — a cross-component diagnostic function that
/// aggregates data from multiple sources (e.g. "powertrain-status",
/// "battery-health").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdFunc {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Component IDs this function aggregates
    #[serde(rename = "sourceComponents")]
    pub source_components: Vec<String>,
}

// ── Areas (ISO 17978-3 §4.2.3) ──────────────────────────────────────────────

/// SOVD area entity — an E/E architecture zone or domain
/// (e.g. "front-left", "powertrain-zone", "adas-domain").
///
/// Gated by `DiscoveryPolicy::areas_enabled()` — MBDS §2.2 forbids this entity type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdArea {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Component IDs belonging to this area
    #[serde(rename = "componentIds")]
    pub component_ids: Vec<String>,
}

// ── Software Packages (SOVD Standard §5.5.10) ────────────────────────────

/// SOVD software package resource with full lifecycle fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdSoftwarePackage {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: SovdSoftwarePackageStatus,
    /// Previous version (for rollback)
    #[serde(skip_serializing_if = "Option::is_none", rename = "previousVersion")]
    pub previous_version: Option<String>,
    /// Installation progress (0–100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Target component ID
    #[serde(skip_serializing_if = "Option::is_none", rename = "componentId")]
    pub component_id: Option<String>,
    /// Timestamp of last status change
    #[serde(skip_serializing_if = "Option::is_none", rename = "updatedAt")]
    pub updated_at: Option<String>,
    /// Error detail (when status == Failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Software package lifecycle states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdSoftwarePackageStatus {
    Available,
    Downloading,
    Downloaded,
    Installing,
    Installed,
    Activated,
    RollingBack,
    Failed,
}

/// Software package upload manifest (metadata for OTA)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdSoftwarePackageManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Download URL (for pull-based OTA)
    #[serde(skip_serializing_if = "Option::is_none", rename = "downloadUrl")]
    pub download_url: Option<String>,
    /// SHA-256 checksum of the package
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Package size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

// ── Bulk Data (SOVD Standard §7.5.3) ─────────────────────────────────────

/// Bulk data categories (MBDS §7.5.3)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdBulkDataCategory {
    CurrentData,
    Logs,
    Trigger,
}

/// Bulk data read request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdBulkReadRequest {
    #[serde(rename = "dataIds")]
    pub data_ids: Vec<String>,
    /// Data category filter (MBDS §7.5.3): currentData, logs, trigger
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<SovdBulkDataCategory>,
}

/// Bulk data read response item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdBulkDataItem {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Bulk data write request item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdBulkWriteItem {
    pub id: String,
    pub value: String,
}

// ── Audit Trail (Wave 1) ─────────────────────────────────────────────────

/// Action classification for audit trail entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SovdAuditAction {
    ReadData,
    WriteData,
    BulkRead,
    BulkWrite,
    ReadFaults,
    ClearFaults,
    ExecuteOperation,
    CancelExecution,
    AcquireLock,
    ReleaseLock,
    ReadConfig,
    WriteConfig,
    ListSoftwarePackages,
    InstallPackage,
    ProximityChallenge,
    SetMode,
    Connect,
    Disconnect,
    AuthSuccess,
    AuthFailure,
    AuthzDenied,
    IoControl,
    FlashStart,
    ReadMemory,
    WriteMemory,
}

/// A single audit trail entry recording a security-relevant action.
///
/// Designed for tamper-resistant logging: each entry is self-contained
/// and can be serialized as a single JSON line for append-only file sinks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdAuditEntry {
    /// Monotonic sequence number
    pub seq: u64,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Authenticated caller identity
    pub caller: String,
    /// Action performed
    pub action: SovdAuditAction,
    /// Target entity (e.g. "component/hpc", "app/health-monitor")
    pub target: String,
    /// Resource path (e.g. "data/rpm", "faults", "lock")
    pub resource: String,
    /// HTTP method
    pub method: String,
    /// Outcome: "success", "denied", "error"
    pub outcome: String,
    /// Optional detail (e.g. error message, written value summary)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// W3C trace ID for correlation with distributed tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// SHA-256 hash of the previous entry (hash chain for tamper detection).
    /// First entry uses `"genesis"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
    /// SHA-256 hash of this entry (computed over all fields except `hash` itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn collection_new_sets_count() {
        let items = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let col = Collection::new(items);
        assert_eq!(col.count, 3);
        assert_eq!(col.value.len(), 3);
    }

    #[test]
    fn collection_empty() {
        let col: Collection<String> = Collection::new(vec![]);
        assert_eq!(col.count, 0);
        assert!(col.value.is_empty());
    }

    #[test]
    fn collection_serializes_odata_count() {
        let col = Collection::new(vec![1, 2]);
        let json = serde_json::to_value(&col).unwrap();
        assert_eq!(json["@odata.count"], 2);
        assert!(json.get("totalItems").is_none());
        assert!(json.get("items").is_none());
        assert!(json.get("value").is_some());
    }

    #[test]
    fn sovd_component_roundtrip() {
        let comp = SovdComponent {
            id: "hpc-main".into(),
            name: "Main HPC".into(),
            category: "ecu".into(),
            description: Some("Test ECU".into()),
            connection_state: SovdConnectionState::Connected,
            software_version: Some("2.1.0".into()),
            hardware_variant: Some("EU-LHD".into()),
            installation_variant: Some("premium".into()),
        };
        let json = serde_json::to_string(&comp).unwrap();
        let deser: SovdComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "hpc-main");
        assert_eq!(deser.connection_state, SovdConnectionState::Connected);
    }

    #[test]
    fn sovd_component_omits_none_description() {
        let comp = SovdComponent {
            id: "x".into(),
            name: "X".into(),
            category: "ecu".into(),
            description: None,
            connection_state: SovdConnectionState::Disconnected,
            software_version: None,
            hardware_variant: None,
            installation_variant: None,
        };
        let json = serde_json::to_value(&comp).unwrap();
        assert!(json.get("description").is_none());
    }

    #[test]
    fn connection_state_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(SovdConnectionState::Connected).unwrap(),
            "connected"
        );
        assert_eq!(
            serde_json::to_value(SovdConnectionState::Disconnected).unwrap(),
            "disconnected"
        );
        assert_eq!(
            serde_json::to_value(SovdConnectionState::Connecting).unwrap(),
            "connecting"
        );
        assert_eq!(
            serde_json::to_value(SovdConnectionState::Error).unwrap(),
            "error"
        );
    }

    #[test]
    fn sovd_fault_roundtrip() {
        let fault = SovdFault {
            id: "f1".into(),
            component_id: "hpc".into(),
            code: "P0123".into(),
            display_code: Some("P0123".into()),
            severity: SovdFaultSeverity::High,
            status: SovdFaultStatus::Active,
            name: "Sensor fault".into(),
            description: None,
            scope: None,
            affected_subsystem: None,
            correlated_signals: vec![],
            classification_tags: vec![],
        };
        let json = serde_json::to_string(&fault).unwrap();
        let deser: SovdFault = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "f1");
        assert_eq!(deser.code, "P0123");
    }

    #[test]
    fn sovd_fault_display_code_serializes_as_camel_case() {
        let fault = SovdFault {
            id: "f1".into(),
            component_id: "hpc".into(),
            code: "P0123".into(),
            display_code: Some("P0123-Display".into()),
            severity: SovdFaultSeverity::High,
            status: SovdFaultStatus::Active,
            name: "Sensor fault".into(),
            description: None,
            scope: None,
            affected_subsystem: None,
            correlated_signals: vec![],
            classification_tags: vec![],
        };
        let json = serde_json::to_value(&fault).unwrap();
        // Must be "displayCode" (camelCase), NOT "display_code" (snake_case)
        assert_eq!(json["displayCode"], "P0123-Display");
        assert!(
            json.get("display_code").is_none(),
            "display_code must not appear in JSON"
        );
    }

    #[test]
    fn sovd_fault_display_code_omitted_when_none() {
        let fault = SovdFault {
            id: "f2".into(),
            component_id: "hpc".into(),
            code: "P0456".into(),
            display_code: None,
            severity: SovdFaultSeverity::Low,
            status: SovdFaultStatus::Passive,
            name: "Minor fault".into(),
            description: None,
            scope: None,
            affected_subsystem: None,
            correlated_signals: vec![],
            classification_tags: vec![],
        };
        let json = serde_json::to_value(&fault).unwrap();
        assert!(
            json.get("displayCode").is_none(),
            "displayCode must be omitted when None"
        );
    }

    #[test]
    fn fault_severity_serializes_lowercase() {
        assert_eq!(serde_json::to_value(SovdFaultSeverity::Low).unwrap(), "low");
        assert_eq!(
            serde_json::to_value(SovdFaultSeverity::Critical).unwrap(),
            "critical"
        );
    }

    #[test]
    fn data_access_serializes_camel_case() {
        assert_eq!(
            serde_json::to_value(SovdDataAccess::ReadOnly).unwrap(),
            "readOnly"
        );
        assert_eq!(
            serde_json::to_value(SovdDataAccess::ReadWrite).unwrap(),
            "readWrite"
        );
        assert_eq!(
            serde_json::to_value(SovdDataAccess::WriteOnly).unwrap(),
            "writeOnly"
        );
    }

    #[test]
    fn sovd_error_response_roundtrip() {
        let envelope = SovdErrorEnvelope {
            error: SovdErrorResponse {
                code: "SOVD-ERR-404".into(),
                message: "Not found".into(),
                target: Some("/components/xyz".into()),
                details: vec![SovdErrorDetail {
                    code: "SOVD-DETAIL".into(),
                    message: "Component missing".into(),
                    target: None,
                }],
                innererror: None,
            },
        };
        let json = serde_json::to_string(&envelope).unwrap();
        // Must have OData error wrapper
        assert!(json.contains("\"error\""));
        assert!(json.contains("\"code\""));
        assert!(json.contains("\"target\""));
        let deser: SovdErrorEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.error.code, "SOVD-ERR-404");
        assert_eq!(deser.error.message, "Not found");
        assert_eq!(deser.error.target.as_deref(), Some("/components/xyz"));
        assert_eq!(deser.error.details.len(), 1);
        assert_eq!(deser.error.details[0].message, "Component missing");
    }

    #[test]
    fn operation_status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(SovdOperationStatus::Idle).unwrap(),
            "idle"
        );
        assert_eq!(
            serde_json::to_value(SovdOperationStatus::Running).unwrap(),
            "running"
        );
        assert_eq!(
            serde_json::to_value(SovdOperationStatus::Completed).unwrap(),
            "completed"
        );
        assert_eq!(
            serde_json::to_value(SovdOperationStatus::Failed).unwrap(),
            "failed"
        );
        assert_eq!(
            serde_json::to_value(SovdOperationStatus::Cancelled).unwrap(),
            "cancelled"
        );
    }

    #[test]
    fn sovd_lock_roundtrip() {
        let lock = SovdLock {
            component_id: "hpc".into(),
            locked_by: "client-1".into(),
            locked_at: "2025-01-01T00:00:00Z".into(),
            expires: Some("2025-01-01T01:00:00Z".into()),
        };
        let json = serde_json::to_string(&lock).unwrap();
        assert!(json.contains("\"componentId\""));
        assert!(json.contains("\"lockedBy\""));
        assert!(json.contains("\"lockedAt\""));
        let deser: SovdLock = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.component_id, "hpc");
        assert_eq!(deser.locked_by, "client-1");
        assert!(deser.expires.is_some());
    }

    #[test]
    fn sovd_lock_omits_none_expires() {
        let lock = SovdLock {
            component_id: "x".into(),
            locked_by: "y".into(),
            locked_at: "t".into(),
            expires: None,
        };
        let json = serde_json::to_value(&lock).unwrap();
        assert!(json.get("expires").is_none());
    }

    #[test]
    fn sovd_capabilities_roundtrip() {
        let caps = SovdCapabilities {
            component_id: "hpc".into(),
            supported_categories: vec!["ecu".into()],
            data_count: 5,
            operation_count: 2,
            features: vec!["data".into(), "faults".into()],
        };
        let json = serde_json::to_string(&caps).unwrap();
        assert!(json.contains("\"supportedCategories\""));
        assert!(json.contains("\"dataCount\""));
        assert!(json.contains("\"operationCount\""));
        let deser: SovdCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.data_count, 5);
        assert_eq!(deser.operation_count, 2);
        assert_eq!(deser.features.len(), 2);
    }

    #[test]
    fn sovd_data_catalog_entry_roundtrip() {
        let entry = SovdDataCatalogEntry {
            id: "F190".into(),
            name: "VIN".into(),
            description: Some("Vehicle Identification Number".into()),
            access: SovdDataAccess::ReadOnly,
            data_type: SovdDataType::String,
            unit: None,
            did: Some("0xF190".into()),
            normal_range: None,
            semantic_ref: Some("Vehicle.Chassis.VIN".into()),
            sampling_hint: None,
            classification_tags: vec![],
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"dataType\""));
        assert!(json.contains("\"x-uds-did\""));
        let deser: SovdDataCatalogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "F190");
        assert_eq!(deser.name, "VIN");
        assert_eq!(deser.data_type, SovdDataType::String);
        assert!(deser.description.is_some());
        assert!(deser.unit.is_none());
    }

    #[test]
    fn sovd_group_roundtrip() {
        let group = SovdGroup {
            id: "powertrain".into(),
            name: "Powertrain".into(),
            description: Some("Engine and transmission".into()),
            component_ids: vec!["hpc".into(), "bms".into()],
        };
        let json = serde_json::to_string(&group).unwrap();
        assert!(json.contains("\"componentIds\""));
        let deser: SovdGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.component_ids.len(), 2);
    }

    #[test]
    fn sovd_proximity_challenge_roundtrip() {
        let ch = SovdProximityChallenge {
            challenge_id: "abc-123".into(),
            status: SovdProximityChallengeStatus::Pending,
            challenge: Some("random-token".into()),
            response: None,
        };
        let json = serde_json::to_string(&ch).unwrap();
        assert!(json.contains("\"challengeId\""));
        let deser: SovdProximityChallenge = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.challenge_id, "abc-123");
        assert!(deser.response.is_none());
    }

    #[test]
    fn proximity_challenge_status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(SovdProximityChallengeStatus::Pending).unwrap(),
            "pending"
        );
        assert_eq!(
            serde_json::to_value(SovdProximityChallengeStatus::Verified).unwrap(),
            "verified"
        );
        assert_eq!(
            serde_json::to_value(SovdProximityChallengeStatus::Failed).unwrap(),
            "failed"
        );
    }

    #[test]
    fn sovd_log_entry_roundtrip() {
        let entry = SovdLogEntry {
            timestamp: "2025-01-01T00:00:00Z".into(),
            level: SovdLogLevel::Warning,
            source: "hpc".into(),
            message: "Timeout detected".into(),
            data: Some(serde_json::json!({"code": 42})),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deser: SovdLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.source, "hpc");
        assert!(deser.data.is_some());
    }

    #[test]
    fn log_level_serializes_lowercase() {
        assert_eq!(serde_json::to_value(SovdLogLevel::Debug).unwrap(), "debug");
        assert_eq!(serde_json::to_value(SovdLogLevel::Info).unwrap(), "info");
        assert_eq!(
            serde_json::to_value(SovdLogLevel::Warning).unwrap(),
            "warning"
        );
        assert_eq!(serde_json::to_value(SovdLogLevel::Error).unwrap(), "error");
    }

    #[test]
    fn sovd_mode_roundtrip() {
        let mode = SovdMode {
            component_id: "hpc".into(),
            current_mode: "default".into(),
            available_modes: vec!["default".into(), "extended".into(), "programming".into()],
            mode_descriptors: vec![SovdModeDescriptor {
                id: "extended".into(),
                name: Some("Extended Diagnostic".into()),
                description: Some("UDS extended session".into()),
                uds_session: Some(0x03),
                requires_security_access: false,
            }],
            active_since: Some("2026-01-01T00:00:00Z".into()),
        };
        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("\"currentMode\""));
        assert!(json.contains("\"availableModes\""));
        assert!(json.contains("\"modeDescriptors\""));
        assert!(json.contains("\"udsSession\""));
        assert!(json.contains("\"activeSince\""));
        let deser: SovdMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.available_modes.len(), 3);
        assert_eq!(deser.mode_descriptors.len(), 1);
        assert_eq!(deser.mode_descriptors[0].uds_session, Some(0x03));
    }

    #[test]
    fn sovd_component_config_roundtrip() {
        let cfg = SovdComponentConfig {
            component_id: "hpc".into(),
            parameters: serde_json::json!({"coding": "FF00", "variant": "01"}),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"componentId\""));
        let deser: SovdComponentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.parameters["coding"], "FF00");
    }

    #[test]
    fn sovd_bulk_data_item_roundtrip() {
        let item = SovdBulkDataItem {
            id: "F190".into(),
            value: Some("4142".into()),
            error: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let deser: SovdBulkDataItem = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.value.unwrap(), "4142");
        assert!(deser.error.is_none());
    }

    #[test]
    fn sovd_bulk_data_item_with_error() {
        let item = SovdBulkDataItem {
            id: "F190".into(),
            value: None,
            error: Some("NRC 0x31".into()),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert!(json.get("value").is_none());
        assert_eq!(json["error"], "NRC 0x31");
    }

    #[test]
    fn sovd_bulk_write_item_roundtrip() {
        let item = SovdBulkWriteItem {
            id: "F190".into(),
            value: "4142".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let deser: SovdBulkWriteItem = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "F190");
        assert_eq!(deser.value, "4142");
    }

    #[test]
    fn sovd_bulk_read_request_roundtrip() {
        let req = SovdBulkReadRequest {
            data_ids: vec!["F190".into(), "F187".into()],
            category: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"dataIds\""));
        let deser: SovdBulkReadRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.data_ids.len(), 2);
        assert!(deser.category.is_none());

        // With category
        let req2 = SovdBulkReadRequest {
            data_ids: vec!["F190".into()],
            category: Some(SovdBulkDataCategory::Logs),
        };
        let json2 = serde_json::to_string(&req2).unwrap();
        assert!(json2.contains("\"category\":\"logs\""));
    }

    #[test]
    fn sovd_operation_execution_roundtrip() {
        let exec = SovdOperationExecution {
            execution_id: "exec-1".into(),
            component_id: "hpc".into(),
            operation_id: "FF00".into(),
            status: SovdOperationStatus::Completed,
            result: Some(serde_json::json!({"data": "01"})),
            progress: Some(100),
            timestamp: Some("2025-01-01T00:00:00Z".into()),
        };
        let json = serde_json::to_string(&exec).unwrap();
        assert!(json.contains("\"executionId\""));
        assert!(json.contains("\"componentId\""));
        assert!(json.contains("\"operationId\""));
        let deser: SovdOperationExecution = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.execution_id, "exec-1");
        assert_eq!(deser.status, SovdOperationStatus::Completed);
        assert_eq!(deser.progress, Some(100));
    }

    #[test]
    fn sovd_operation_execution_omits_none_fields() {
        let exec = SovdOperationExecution {
            execution_id: "e1".into(),
            component_id: "c1".into(),
            operation_id: "o1".into(),
            status: SovdOperationStatus::Running,
            result: None,
            progress: None,
            timestamp: None,
        };
        let json = serde_json::to_value(&exec).unwrap();
        assert!(json.get("result").is_none());
        assert!(json.get("progress").is_none());
        assert!(json.get("timestamp").is_none());
    }

    // ── T4.1: Schema Stability Regression Tests ──────────────────────────
    //
    // These tests verify that the JSON shapes of SovdDataCatalogEntry and
    // SovdFault haven't changed without a schema version bump. If a field
    // is added/removed/renamed, these tests will fail, signaling that
    // downstream ML pipelines may break.

    #[test]
    fn t4_1_data_catalog_entry_schema_stability() {
        let entry = SovdDataCatalogEntry {
            id: "test".into(),
            name: "Test Data".into(),
            description: Some("A test data item".into()),
            access: SovdDataAccess::ReadOnly,
            data_type: SovdDataType::Float,
            unit: Some("V".into()),
            did: Some("0xF190".into()),
            normal_range: Some(crate::data_catalog::NormalRange {
                min: 0.0,
                max: 16.0,
            }),
            semantic_ref: Some("Vehicle.Powertrain.Battery.Voltage".into()),
            sampling_hint: Some(1.0),
            classification_tags: vec!["powertrain".into()],
        };
        let json = serde_json::to_value(&entry).unwrap();

        // Required fields — MUST always be present
        assert!(json.get("id").is_some(), "schema break: 'id' missing");
        assert!(json.get("name").is_some(), "schema break: 'name' missing");
        assert!(
            json.get("access").is_some(),
            "schema break: 'access' missing"
        );
        assert!(
            json.get("dataType").is_some(),
            "schema break: 'dataType' missing"
        );

        // Wave 4 semantic fields — MUST be present when populated
        assert!(
            json.get("normalRange").is_some(),
            "schema break: 'normalRange' missing"
        );
        assert!(
            json.get("semanticRef").is_some(),
            "schema break: 'semanticRef' missing"
        );
        assert!(
            json.get("samplingHint").is_some(),
            "schema break: 'samplingHint' missing"
        );
        assert!(
            json.get("classificationTags").is_some(),
            "schema break: 'classificationTags' missing"
        );

        // Verify normalRange shape
        let nr = json.get("normalRange").unwrap();
        assert!(
            nr.get("min").is_some(),
            "schema break: normalRange.min missing"
        );
        assert!(
            nr.get("max").is_some(),
            "schema break: normalRange.max missing"
        );

        // Verify camelCase naming (not snake_case)
        assert!(
            json.get("data_type").is_none(),
            "schema break: 'data_type' should be 'dataType'"
        );
        assert!(
            json.get("normal_range").is_none(),
            "schema break: 'normal_range' should be 'normalRange'"
        );
        assert!(
            json.get("semantic_ref").is_none(),
            "schema break: 'semantic_ref' should be 'semanticRef'"
        );
        assert!(
            json.get("sampling_hint").is_none(),
            "schema break: 'sampling_hint' should be 'samplingHint'"
        );
        assert!(
            json.get("classification_tags").is_none(),
            "schema break: 'classification_tags' should be 'classificationTags'"
        );
    }

    #[test]
    fn t4_1_data_catalog_entry_optional_fields_omitted() {
        let entry = SovdDataCatalogEntry {
            id: "minimal".into(),
            name: "Minimal".into(),
            description: None,
            access: SovdDataAccess::ReadOnly,
            data_type: SovdDataType::Integer,
            unit: None,
            did: None,
            normal_range: None,
            semantic_ref: None,
            sampling_hint: None,
            classification_tags: vec![],
        };
        let json = serde_json::to_value(&entry).unwrap();

        // Optional fields MUST be omitted when None/empty (skip_serializing_if)
        assert!(
            json.get("description").is_none(),
            "schema break: empty description should be omitted"
        );
        assert!(
            json.get("unit").is_none(),
            "schema break: empty unit should be omitted"
        );
        assert!(
            json.get("x-uds-did").is_none(),
            "schema break: empty did should be omitted"
        );
        assert!(
            json.get("normalRange").is_none(),
            "schema break: empty normalRange should be omitted"
        );
        assert!(
            json.get("semanticRef").is_none(),
            "schema break: empty semanticRef should be omitted"
        );
        assert!(
            json.get("samplingHint").is_none(),
            "schema break: empty samplingHint should be omitted"
        );
        assert!(
            json.get("classificationTags").is_none(),
            "schema break: empty classificationTags should be omitted"
        );
    }

    #[test]
    fn t4_1_fault_schema_stability() {
        let fault = SovdFault {
            id: "f1".into(),
            component_id: "hpc".into(),
            code: "P0123".into(),
            display_code: Some("P0123".into()),
            severity: SovdFaultSeverity::High,
            status: SovdFaultStatus::Active,
            name: "Test Fault".into(),
            description: Some("A test fault".into()),
            scope: Some("component".into()),
            affected_subsystem: Some("powertrain".into()),
            correlated_signals: vec!["Vehicle.Speed".into()],
            classification_tags: vec!["emission".into()],
        };
        let json = serde_json::to_value(&fault).unwrap();

        // Required fields
        assert!(json.get("id").is_some(), "schema break: 'id' missing");
        assert!(
            json.get("componentId").is_some(),
            "schema break: 'componentId' missing"
        );
        assert!(json.get("code").is_some(), "schema break: 'code' missing");
        assert!(
            json.get("severity").is_some(),
            "schema break: 'severity' missing"
        );
        assert!(
            json.get("status").is_some(),
            "schema break: 'status' missing"
        );
        assert!(json.get("name").is_some(), "schema break: 'name' missing");

        // Wave 4 ontology enrichment fields
        assert!(
            json.get("affectedSubsystem").is_some(),
            "schema break: 'affectedSubsystem' missing"
        );
        assert!(
            json.get("correlatedSignals").is_some(),
            "schema break: 'correlatedSignals' missing"
        );
        assert!(
            json.get("classificationTags").is_some(),
            "schema break: 'classificationTags' missing"
        );

        // Verify camelCase
        assert!(
            json.get("component_id").is_none(),
            "schema break: should be 'componentId'"
        );
        assert!(
            json.get("display_code").is_none(),
            "schema break: should be 'displayCode'"
        );
        assert!(
            json.get("affected_subsystem").is_none(),
            "schema break: should be 'affectedSubsystem'"
        );
        assert!(
            json.get("correlated_signals").is_none(),
            "schema break: should be 'correlatedSignals'"
        );
        assert!(
            json.get("classification_tags").is_none(),
            "schema break: should be 'classificationTags'"
        );
    }

    #[test]
    fn t4_1_fault_optional_fields_omitted() {
        let fault = SovdFault {
            id: "f2".into(),
            component_id: "hpc".into(),
            code: "P0456".into(),
            display_code: None,
            severity: SovdFaultSeverity::Low,
            status: SovdFaultStatus::Passive,
            name: "Minimal fault".into(),
            description: None,
            scope: None,
            affected_subsystem: None,
            correlated_signals: vec![],
            classification_tags: vec![],
        };
        let json = serde_json::to_value(&fault).unwrap();
        assert!(
            json.get("displayCode").is_none(),
            "empty displayCode should be omitted"
        );
        assert!(
            json.get("description").is_none(),
            "empty description should be omitted"
        );
        assert!(json.get("scope").is_none(), "empty scope should be omitted");
        assert!(
            json.get("affectedSubsystem").is_none(),
            "empty affectedSubsystem should be omitted"
        );
        assert!(
            json.get("correlatedSignals").is_none(),
            "empty correlatedSignals should be omitted"
        );
        assert!(
            json.get("classificationTags").is_none(),
            "empty classificationTags should be omitted"
        );
    }
}
