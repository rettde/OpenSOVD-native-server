// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// UDS Manager — session control, security access, DID read/write, routine control,
// DTC operations, firmware transfer (0x34/36/37), TesterPresent keepalive
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info};

use native_interfaces::{
    ConnectionError, DiagServiceError, DiagTransport, DiagnosticSession, UdsResponse,
};

/// UDS Manager — manages UDS communication over a DoIP connection.
/// Handles session tracking and keepalive.
pub struct UdsManager {
    transport: Arc<dyn DiagTransport>,
    current_session: Arc<RwLock<DiagnosticSession>>,
    security_unlocked: Arc<RwLock<bool>>,
}

impl UdsManager {
    pub fn new(transport: Arc<dyn DiagTransport>) -> Self {
        Self {
            transport,
            current_session: Arc::new(RwLock::new(DiagnosticSession::Default)),
            security_unlocked: Arc::new(RwLock::new(false)),
        }
    }

    pub fn transport(&self) -> &Arc<dyn DiagTransport> {
        &self.transport
    }

    /// Send a raw UDS request and get the response payload
    async fn send_request(&self, request: &[u8]) -> Result<Vec<u8>, DiagServiceError> {
        let response = self
            .transport
            .send_diagnostic(request)
            .await
            .map_err(|e| match e {
                ConnectionError::Timeout(_) => DiagServiceError::Timeout,
                ConnectionError::Closed => {
                    DiagServiceError::ConnectionClosed("DoIP connection closed".to_owned())
                }
                other => DiagServiceError::SendFailed(other.to_string()),
            })?;

        match response {
            UdsResponse::Message(payload) => {
                // Check for NRC (negative response)
                if payload.data.len() >= 3 && payload.data[0] == 0x7F {
                    let nrc = payload.data[2];
                    return Err(DiagServiceError::Nack(nrc));
                }
                Ok(payload.data)
            }
            UdsResponse::ResponsePending(_) => Err(DiagServiceError::Timeout),
            UdsResponse::BusyRepeatRequest(_) => Err(DiagServiceError::ResourceError(
                "ECU busy, repeat request".to_owned(),
            )),
            UdsResponse::TemporarilyNotAvailable(_) => Err(DiagServiceError::ResourceError(
                "ECU temporarily not available".to_owned(),
            )),
            UdsResponse::TesterPresentNRC(nrc) => Err(DiagServiceError::Nack(nrc)),
        }
    }

    // ── Service 0x10: DiagnosticSessionControl ──────────────────────────────

    #[tracing::instrument(skip(self), fields(session = ?session))]
    pub async fn diagnostic_session_control(
        &self,
        session: DiagnosticSession,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let request = vec![0x10, session as u8];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x50) {
            return Err(DiagServiceError::UnexpectedResponse(Some(format!(
                "Expected SID 0x50, got 0x{:02X}",
                response.first().unwrap_or(&0)
            ))));
        }

        *self.current_session.write().await = session;
        info!("Session switched to {:?}", session);
        Ok(response)
    }

    // ── Service 0x11: ECUReset ──────────────────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn ecu_reset(&self, reset_type: u8) -> Result<Vec<u8>, DiagServiceError> {
        let request = vec![0x11, reset_type];
        self.send_request(&request).await
    }

    // ── Service 0x22: ReadDataByIdentifier ──────────────────────────────────

    #[tracing::instrument(skip(self), fields(did = %format!("0x{did:04X}")))]
    pub async fn read_data_by_identifier(&self, did: u16) -> Result<Vec<u8>, DiagServiceError> {
        let request = vec![0x22, (did >> 8) as u8, (did & 0xFF) as u8];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x62) || response.len() < 3 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid ReadDataByIdentifier response".to_owned(),
            )));
        }

        // Skip SID (0x62) + DID (2 bytes) = data starts at offset 3
        Ok(response[3..].to_vec())
    }

    // ── Service 0x2E: WriteDataByIdentifier ─────────────────────────────────

    #[tracing::instrument(skip(self, value), fields(did = %format!("0x{did:04X}")))]
    pub async fn write_data_by_identifier(
        &self,
        did: u16,
        value: &[u8],
    ) -> Result<(), DiagServiceError> {
        let mut request = vec![0x2E, (did >> 8) as u8, (did & 0xFF) as u8];
        request.extend_from_slice(value);
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x6E) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected positive response 0x6E for WriteDataByIdentifier".to_owned(),
            )));
        }
        Ok(())
    }

    // ── Service 0x19: ReadDTCInformation ────────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn read_dtc_by_status_mask(
        &self,
        status_mask: u8,
    ) -> Result<Vec<DtcInfo>, DiagServiceError> {
        let request = vec![0x19, 0x02, status_mask];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x59) || response.len() < 3 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid ReadDTCInformation response".to_owned(),
            )));
        }

        // Parse DTC records: each is 4 bytes (3 DTC + 1 status)
        let dtc_data = &response[3..];
        let mut dtcs = Vec::new();
        for chunk in dtc_data.chunks_exact(4) {
            dtcs.push(DtcInfo {
                dtc_high: chunk[0],
                dtc_mid: chunk[1],
                dtc_low: chunk[2],
                status_mask: chunk[3],
            });
        }

        debug!(count = dtcs.len(), "DTCs read");
        Ok(dtcs)
    }

    // ── Service 0x14: ClearDiagnosticInformation ────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn clear_dtc(&self, group: u32) -> Result<(), DiagServiceError> {
        let request = vec![
            0x14,
            ((group >> 16) & 0xFF) as u8,
            ((group >> 8) & 0xFF) as u8,
            (group & 0xFF) as u8,
        ];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x54) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected positive response 0x54 for ClearDTC".to_owned(),
            )));
        }
        info!("DTCs cleared (group=0x{group:06X})");
        Ok(())
    }

    // ── Service 0x27: SecurityAccess ────────────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn security_access_request_seed(
        &self,
        level: u8,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let request = vec![0x27, level];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x67) || response.len() < 2 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid SecurityAccess seed response".to_owned(),
            )));
        }
        Ok(response[2..].to_vec())
    }

    #[tracing::instrument(skip(self, key))]
    pub async fn security_access_send_key(
        &self,
        level: u8,
        key: &[u8],
    ) -> Result<(), DiagServiceError> {
        let mut request = vec![0x27, level];
        request.extend_from_slice(key);
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x67) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "SecurityAccess key rejected".to_owned(),
            )));
        }
        *self.security_unlocked.write().await = true;
        info!("Security access granted (level=0x{level:02X})");
        Ok(())
    }

    // ── Service 0x31: RoutineControl ────────────────────────────────────────

    #[tracing::instrument(skip(self, params), fields(routine = %format!("0x{routine_id:04X}")))]
    pub async fn routine_control_start(
        &self,
        routine_id: u16,
        params: Option<&[u8]>,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let mut request = vec![
            0x31,
            0x01,
            (routine_id >> 8) as u8,
            (routine_id & 0xFF) as u8,
        ];
        if let Some(p) = params {
            request.extend_from_slice(p);
        }
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x71) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid RoutineControl response".to_owned(),
            )));
        }
        Ok(if response.len() > 4 {
            response[4..].to_vec()
        } else {
            vec![]
        })
    }

    // ── Service 0x2F: InputOutputControlByIdentifier ────────────────────────

    #[tracing::instrument(skip(self, control_option_record), fields(did = %format!("0x{did:04X}")))]
    pub async fn input_output_control(
        &self,
        did: u16,
        control_param: IoControlParameter,
        control_option_record: Option<&[u8]>,
    ) -> Result<Vec<u8>, DiagServiceError> {
        let mut request = vec![
            0x2F,
            (did >> 8) as u8,
            (did & 0xFF) as u8,
            control_param as u8,
        ];
        if let Some(data) = control_option_record {
            request.extend_from_slice(data);
        }
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x6F) || response.len() < 4 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid InputOutputControl response".to_owned(),
            )));
        }

        // Return status record (bytes after SID + DID + controlParam)
        Ok(response[4..].to_vec())
    }

    // ── Service 0x28: CommunicationControl ────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn communication_control(
        &self,
        control_type: CommControlType,
        communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        let request = vec![0x28, control_type as u8, communication_type];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0x68) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected positive response 0x68 for CommunicationControl".to_owned(),
            )));
        }
        info!(
            "CommunicationControl: type={:?}, comm=0x{communication_type:02X}",
            control_type
        );
        Ok(())
    }

    // ── Service 0x85: ControlDTCSetting ───────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn control_dtc_setting(
        &self,
        setting_type: DtcSettingType,
    ) -> Result<(), DiagServiceError> {
        let request = vec![0x85, setting_type as u8];
        let response = self.send_request(&request).await?;

        if response.first() != Some(&0xC5) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected positive response 0xC5 for ControlDTCSetting".to_owned(),
            )));
        }
        info!("DTC setting changed to {:?}", setting_type);
        Ok(())
    }

    // ── Service 0x23: ReadMemoryByAddress ─────────────────────────────────

    #[tracing::instrument(skip(self), fields(addr = %format!("0x{address:08X}"), size = size))]
    pub async fn read_memory_by_address(
        &self,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        // addressAndLengthFormatIdentifier: 4 bytes address, 4 bytes size
        let addr_len_format: u8 = 0x44;
        let mut request = vec![0x23, addr_len_format];
        request.extend_from_slice(&address.to_be_bytes());
        request.extend_from_slice(&size.to_be_bytes());

        let response = self.send_request(&request).await?;
        if response.first() != Some(&0x63) || response.len() < 2 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid ReadMemoryByAddress response".to_owned(),
            )));
        }

        // Data starts after SID byte
        Ok(response[1..].to_vec())
    }

    // ── Service 0x3D: WriteMemoryByAddress ────────────────────────────────

    #[tracing::instrument(skip(self, data), fields(addr = %format!("0x{address:08X}"), size = data.len()))]
    pub async fn write_memory_by_address(
        &self,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        let data_len = data.len() as u32;
        let addr_len_format: u8 = 0x44;
        let mut request = vec![0x3D, addr_len_format];
        request.extend_from_slice(&address.to_be_bytes());
        request.extend_from_slice(&data_len.to_be_bytes());
        request.extend_from_slice(data);

        let response = self.send_request(&request).await?;
        if response.first() != Some(&0x7D) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected positive response 0x7D for WriteMemoryByAddress".to_owned(),
            )));
        }
        info!("Memory written at 0x{address:08X} ({} bytes)", data.len());
        Ok(())
    }

    // ── Service 0x3E: TesterPresent ─────────────────────────────────────────

    #[tracing::instrument(skip(self))]
    pub async fn tester_present(&self, suppress_response: bool) -> Result<(), DiagServiceError> {
        let sub = if suppress_response { 0x80 } else { 0x00 };
        let request = vec![0x3E, sub];

        if suppress_response {
            let _ = self.transport.send_diagnostic(&request).await;
            Ok(())
        } else {
            let response = self.send_request(&request).await?;
            if response.first() != Some(&0x7E) {
                return Err(DiagServiceError::UnexpectedResponse(Some(
                    "Expected 0x7E for TesterPresent".to_owned(),
                )));
            }
            Ok(())
        }
    }

    // ── Service 0x34/0x36/0x37: Firmware Transfer ───────────────────────────

    #[tracing::instrument(skip(self), fields(addr = %format!("0x{memory_address:08X}"), size = memory_size))]
    pub async fn request_download(
        &self,
        memory_address: u32,
        memory_size: u32,
        compression: u8,
        encrypting: u8,
    ) -> Result<u32, DiagServiceError> {
        let data_format = (compression << 4) | encrypting;
        let addr_and_len_format: u8 = 0x44; // 4 bytes address, 4 bytes length

        let mut request = vec![0x34, data_format, addr_and_len_format];
        request.extend_from_slice(&memory_address.to_be_bytes());
        request.extend_from_slice(&memory_size.to_be_bytes());

        let response = self.send_request(&request).await?;
        if response.first() != Some(&0x74) || response.len() < 3 {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Invalid RequestDownload response".to_owned(),
            )));
        }

        let len_field_size = ((response[1] >> 4) & 0x0F) as usize;
        let mut max_block = 0u32;
        for i in 0..len_field_size.min(4) {
            if let Some(&byte) = response.get(2 + i) {
                max_block = (max_block << 8) | byte as u32;
            }
        }

        info!(max_block, "Download accepted");
        Ok(max_block)
    }

    #[tracing::instrument(skip(self, data), fields(block = block_sequence, len = data.len()))]
    pub async fn transfer_data(
        &self,
        block_sequence: u8,
        data: &[u8],
    ) -> Result<(), DiagServiceError> {
        let mut request = vec![0x36, block_sequence];
        request.extend_from_slice(data);

        let response = self.send_request(&request).await?;
        if response.first() != Some(&0x76) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected 0x76 for TransferData".to_owned(),
            )));
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub async fn request_transfer_exit(&self) -> Result<(), DiagServiceError> {
        let request = vec![0x37];
        let response = self.send_request(&request).await?;
        if response.first() != Some(&0x77) {
            return Err(DiagServiceError::UnexpectedResponse(Some(
                "Expected 0x77 for RequestTransferExit".to_owned(),
            )));
        }
        info!("Transfer completed");
        Ok(())
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    pub async fn current_session(&self) -> DiagnosticSession {
        *self.current_session.read().await
    }

    pub async fn is_security_unlocked(&self) -> bool {
        *self.security_unlocked.read().await
    }
}

/// DTC information from ReadDTCInformation
#[derive(Debug, Clone)]
pub struct DtcInfo {
    pub dtc_high: u8,
    pub dtc_mid: u8,
    pub dtc_low: u8,
    pub status_mask: u8,
}

impl DtcInfo {
    pub fn to_dtc_string(&self) -> String {
        format!(
            "{:02X}{:02X}{:02X}",
            self.dtc_high, self.dtc_mid, self.dtc_low
        )
    }

    pub fn is_active(&self) -> bool {
        self.status_mask & 0x01 != 0
    }

    pub fn is_confirmed(&self) -> bool {
        self.status_mask & 0x08 != 0
    }

    pub fn is_pending(&self) -> bool {
        self.status_mask & 0x04 != 0
    }
}

/// InputOutputControl parameter — ISO 14229 SID 0x2F sub-function
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IoControlParameter {
    /// Return control to ECU
    ReturnControlToEcu = 0x00,
    /// Reset to default
    ResetToDefault = 0x01,
    /// Freeze current state
    FreezeCurrentState = 0x02,
    /// Short-term adjustment
    ShortTermAdjustment = 0x03,
}

/// CommunicationControl sub-function — ISO 14229 SID 0x28
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CommControlType {
    EnableRxAndTx = 0x00,
    EnableRxAndDisableTx = 0x01,
    DisableRxAndEnableTx = 0x02,
    DisableRxAndTx = 0x03,
}

/// ControlDTCSetting sub-function — ISO 14229 SID 0x85
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DtcSettingType {
    On = 0x01,
    Off = 0x02,
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests — mock-based, no network required
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use native_interfaces::{DiagTransport, ServicePayload};
    use std::sync::Mutex;

    /// Mock transport that records sent requests and replays scripted responses
    struct MockTransport {
        /// Recorded requests (for verification)
        requests: Mutex<Vec<Vec<u8>>>,
        /// Scripted responses (FIFO)
        responses: Mutex<Vec<Result<UdsResponse, ConnectionError>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Result<UdsResponse, ConnectionError>>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(responses),
            }
        }

        /// Helper: build a positive UDS response wrapped in DoIP
        fn positive(data: Vec<u8>) -> Result<UdsResponse, ConnectionError> {
            Ok(UdsResponse::Message(ServicePayload {
                data,
                source_address: 0x0001,
                target_address: 0x0E00,
            }))
        }

        /// Helper: build a negative UDS response (NRC)
        fn nrc(sid: u8, nrc_code: u8) -> Result<UdsResponse, ConnectionError> {
            Ok(UdsResponse::Message(ServicePayload {
                data: vec![0x7F, sid, nrc_code],
                source_address: 0x0001,
                target_address: 0x0E00,
            }))
        }

        fn last_request(&self) -> Vec<u8> {
            self.requests
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl DiagTransport for MockTransport {
        async fn send_diagnostic(&self, data: &[u8]) -> Result<UdsResponse, ConnectionError> {
            self.requests.lock().unwrap().push(data.to_vec());
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn uds(
        responses: Vec<Result<UdsResponse, ConnectionError>>,
    ) -> (Arc<UdsManager>, Arc<MockTransport>) {
        let transport = Arc::new(MockTransport::new(responses));
        let mgr = Arc::new(UdsManager::new(transport.clone() as Arc<dyn DiagTransport>));
        (mgr, transport)
    }

    // ── DtcInfo unit tests (pure logic) ─────────────────────────────────

    #[test]
    fn dtc_info_to_string() {
        let dtc = DtcInfo {
            dtc_high: 0x01,
            dtc_mid: 0x23,
            dtc_low: 0x45,
            status_mask: 0x09,
        };
        assert_eq!(dtc.to_dtc_string(), "012345");
    }

    #[test]
    fn dtc_info_is_active() {
        let dtc = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x01,
        };
        assert!(dtc.is_active());
        let dtc2 = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x08,
        };
        assert!(!dtc2.is_active());
    }

    #[test]
    fn dtc_info_is_confirmed() {
        let dtc = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x09,
        };
        assert!(dtc.is_confirmed());
        let dtc2 = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x01,
        };
        assert!(!dtc2.is_confirmed());
    }

    #[test]
    fn dtc_info_is_pending() {
        let dtc = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x04,
        };
        assert!(dtc.is_pending());
        let dtc2 = DtcInfo {
            dtc_high: 0,
            dtc_mid: 0,
            dtc_low: 0,
            status_mask: 0x01,
        };
        assert!(!dtc2.is_pending());
    }

    // ── Session control (SID 0x10) ──────────────────────────────────────

    #[tokio::test]
    async fn session_control_sends_correct_frame() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x50, 0x03])]);
        mgr.diagnostic_session_control(DiagnosticSession::Extended)
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x10, 0x03]);
        assert_eq!(mgr.current_session().await, DiagnosticSession::Extended);
    }

    #[tokio::test]
    async fn session_control_programming() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x50, 0x02])]);
        mgr.diagnostic_session_control(DiagnosticSession::Programming)
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x10, 0x02]);
        assert_eq!(mgr.current_session().await, DiagnosticSession::Programming);
    }

    #[tokio::test]
    async fn session_control_nrc_rejected() {
        let (mgr, _) = uds(vec![MockTransport::nrc(0x10, 0x22)]);
        let err = mgr
            .diagnostic_session_control(DiagnosticSession::Extended)
            .await
            .unwrap_err();
        assert!(matches!(err, DiagServiceError::Nack(0x22)));
    }

    #[tokio::test]
    async fn session_control_unexpected_sid() {
        let (mgr, _) = uds(vec![MockTransport::positive(vec![0x99])]);
        let err = mgr
            .diagnostic_session_control(DiagnosticSession::Default)
            .await
            .unwrap_err();
        assert!(matches!(err, DiagServiceError::UnexpectedResponse(_)));
    }

    // ── Read data (SID 0x22) ────────────────────────────────────────────

    #[tokio::test]
    async fn read_data_sends_correct_frame() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![
            0x62, 0xF1, 0x90, 0x41, 0x42,
        ])]);
        let data = mgr.read_data_by_identifier(0xF190).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x22, 0xF1, 0x90]);
        assert_eq!(data, vec![0x41, 0x42]);
    }

    #[tokio::test]
    async fn read_data_strips_header() {
        let (mgr, _) = uds(vec![MockTransport::positive(vec![
            0x62, 0x02, 0x00, 0xDE, 0xAD, 0xBE, 0xEF,
        ])]);
        let data = mgr.read_data_by_identifier(0x0200).await.unwrap();
        assert_eq!(data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[tokio::test]
    async fn read_data_nrc() {
        let (mgr, _) = uds(vec![MockTransport::nrc(0x22, 0x31)]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::Nack(0x31)));
    }

    #[tokio::test]
    async fn read_data_too_short_response() {
        let (mgr, _) = uds(vec![MockTransport::positive(vec![0x62, 0xF1])]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::UnexpectedResponse(_)));
    }

    // ── Write data (SID 0x2E) ───────────────────────────────────────────

    #[tokio::test]
    async fn write_data_sends_correct_frame() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x6E, 0xF1, 0x90])]);
        mgr.write_data_by_identifier(0xF190, &[0x41, 0x42])
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x2E, 0xF1, 0x90, 0x41, 0x42]);
    }

    #[tokio::test]
    async fn write_data_nrc() {
        let (mgr, _) = uds(vec![MockTransport::nrc(0x2E, 0x72)]);
        let err = mgr
            .write_data_by_identifier(0xF190, &[0x01])
            .await
            .unwrap_err();
        assert!(matches!(err, DiagServiceError::Nack(0x72)));
    }

    // ── Read DTC (SID 0x19) ─────────────────────────────────────────────

    #[tokio::test]
    async fn read_dtc_parses_records() {
        // SID 0x59, sub 0x02, availability mask, then 2 DTCs × 4 bytes
        let resp = vec![
            0x59, 0x02, 0xFF, 0x01, 0x23, 0x45, 0x09, 0xAB, 0xCD, 0xEF, 0x04,
        ];
        let (mgr, mock) = uds(vec![MockTransport::positive(resp)]);
        let dtcs = mgr.read_dtc_by_status_mask(0xFF).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x19, 0x02, 0xFF]);
        assert_eq!(dtcs.len(), 2);
        assert_eq!(dtcs[0].to_dtc_string(), "012345");
        assert!(dtcs[0].is_active());
        assert!(dtcs[0].is_confirmed());
        assert_eq!(dtcs[1].to_dtc_string(), "ABCDEF");
        assert!(dtcs[1].is_pending());
    }

    #[tokio::test]
    async fn read_dtc_empty_response() {
        let (mgr, _) = uds(vec![MockTransport::positive(vec![0x59, 0x02, 0xFF])]);
        let dtcs = mgr.read_dtc_by_status_mask(0xFF).await.unwrap();
        assert!(dtcs.is_empty());
    }

    // ── Clear DTC (SID 0x14) ────────────────────────────────────────────

    #[tokio::test]
    async fn clear_dtc_all_groups() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x54])]);
        mgr.clear_dtc(0xFFFFFF).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x14, 0xFF, 0xFF, 0xFF]);
    }

    #[tokio::test]
    async fn clear_dtc_specific_group() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x54])]);
        mgr.clear_dtc(0x010203).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x14, 0x01, 0x02, 0x03]);
    }

    // ── Security access (SID 0x27) ──────────────────────────────────────

    #[tokio::test]
    async fn security_access_request_seed() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![
            0x67, 0x01, 0xAA, 0xBB, 0xCC, 0xDD,
        ])]);
        let seed = mgr.security_access_request_seed(0x01).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x27, 0x01]);
        assert_eq!(seed, vec![0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[tokio::test]
    async fn security_access_send_key_unlocks() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x67, 0x02])]);
        mgr.security_access_send_key(0x02, &[0x11, 0x22])
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x27, 0x02, 0x11, 0x22]);
        assert!(mgr.is_security_unlocked().await);
    }

    #[tokio::test]
    async fn security_access_wrong_key_nrc() {
        let (mgr, _) = uds(vec![MockTransport::nrc(0x27, 0x35)]);
        let err = mgr
            .security_access_send_key(0x02, &[0xFF])
            .await
            .unwrap_err();
        assert!(matches!(err, DiagServiceError::Nack(0x35)));
        assert!(!mgr.is_security_unlocked().await);
    }

    // ── Routine control (SID 0x31) ──────────────────────────────────────

    #[tokio::test]
    async fn routine_control_start_no_params() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x71, 0x01, 0xFF, 0x00])]);
        let result = mgr.routine_control_start(0xFF00, None).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x31, 0x01, 0xFF, 0x00]);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn routine_control_start_with_params_and_result() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![
            0x71, 0x01, 0xFF, 0x01, 0xCA, 0xFE,
        ])]);
        let result = mgr
            .routine_control_start(0xFF01, Some(&[0x01, 0x02]))
            .await
            .unwrap();
        assert_eq!(
            mock.last_request(),
            vec![0x31, 0x01, 0xFF, 0x01, 0x01, 0x02]
        );
        assert_eq!(result, vec![0xCA, 0xFE]);
    }

    // ── ECU Reset (SID 0x11) ────────────────────────────────────────────

    #[tokio::test]
    async fn ecu_reset_hard() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x51, 0x01])]);
        mgr.ecu_reset(0x01).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x11, 0x01]);
    }

    // ── TesterPresent (SID 0x3E) ────────────────────────────────────────

    #[tokio::test]
    async fn tester_present_with_response() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x7E, 0x00])]);
        mgr.tester_present(false).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x3E, 0x00]);
    }

    #[tokio::test]
    async fn tester_present_suppress_response() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x7E, 0x80])]);
        mgr.tester_present(true).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x3E, 0x80]);
    }

    // ── IO Control (SID 0x2F) ───────────────────────────────────────────

    #[tokio::test]
    async fn io_control_return_to_ecu() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![
            0x6F, 0x20, 0x00, 0x00, 0x42,
        ])]);
        let result = mgr
            .input_output_control(0x2000, IoControlParameter::ReturnControlToEcu, None)
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x2F, 0x20, 0x00, 0x00]);
        assert_eq!(result, vec![0x42]);
    }

    #[tokio::test]
    async fn io_control_short_term_adjustment() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![
            0x6F, 0x20, 0x00, 0x03, 0xFF,
        ])]);
        let result = mgr
            .input_output_control(
                0x2000,
                IoControlParameter::ShortTermAdjustment,
                Some(&[0xAA]),
            )
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x2F, 0x20, 0x00, 0x03, 0xAA]);
        assert_eq!(result, vec![0xFF]);
    }

    // ── Communication control (SID 0x28) ────────────────────────────────

    #[tokio::test]
    async fn communication_control_disable_tx() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x68, 0x01])]);
        mgr.communication_control(CommControlType::EnableRxAndDisableTx, 0x01)
            .await
            .unwrap();
        assert_eq!(mock.last_request(), vec![0x28, 0x01, 0x01]);
    }

    // ── DTC Setting (SID 0x85) ──────────────────────────────────────────

    #[tokio::test]
    async fn dtc_setting_off() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0xC5, 0x02])]);
        mgr.control_dtc_setting(DtcSettingType::Off).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x85, 0x02]);
    }

    // ── Read Memory (SID 0x23) ──────────────────────────────────────────

    #[tokio::test]
    async fn read_memory_by_address() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x63, 0xDE, 0xAD])]);
        let data = mgr.read_memory_by_address(0x20000000, 2).await.unwrap();
        // Request: 0x23 + format(0x44) + 4-byte addr + 4-byte size
        let req = mock.last_request();
        assert_eq!(req[0], 0x23);
        assert_eq!(req[1], 0x44);
        assert_eq!(&req[2..6], &[0x20, 0x00, 0x00, 0x00]);
        assert_eq!(&req[6..10], &[0x00, 0x00, 0x00, 0x02]);
        assert_eq!(data, vec![0xDE, 0xAD]);
    }

    // ── Write Memory (SID 0x3D) ─────────────────────────────────────────

    #[tokio::test]
    async fn write_memory_by_address() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x7D])]);
        mgr.write_memory_by_address(0x10000000, &[0xCA, 0xFE])
            .await
            .unwrap();
        let req = mock.last_request();
        assert_eq!(req[0], 0x3D);
        assert_eq!(req[1], 0x44);
        assert_eq!(&req[2..6], &[0x10, 0x00, 0x00, 0x00]);
        assert_eq!(&req[6..10], &[0x00, 0x00, 0x00, 0x02]);
        assert_eq!(&req[10..], &[0xCA, 0xFE]);
    }

    // ── Firmware transfer (SID 0x34/0x36/0x37) ─────────────────────────

    #[tokio::test]
    async fn request_download_parses_max_block() {
        // Response: 0x74, lengthFormatId=0x20 (2 bytes), maxBlock=0x0400 (1024)
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x74, 0x20, 0x04, 0x00])]);
        let max_block = mgr
            .request_download(0x20000000, 0x10000, 0, 0)
            .await
            .unwrap();
        let req = mock.last_request();
        assert_eq!(req[0], 0x34);
        assert_eq!(max_block, 0x0400);
    }

    #[tokio::test]
    async fn transfer_data_sends_block() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x76, 0x01])]);
        mgr.transfer_data(0x01, &[0xAA, 0xBB, 0xCC]).await.unwrap();
        assert_eq!(mock.last_request(), vec![0x36, 0x01, 0xAA, 0xBB, 0xCC]);
    }

    #[tokio::test]
    async fn request_transfer_exit() {
        let (mgr, mock) = uds(vec![MockTransport::positive(vec![0x77])]);
        mgr.request_transfer_exit().await.unwrap();
        assert_eq!(mock.last_request(), vec![0x37]);
    }

    // ── Transport error mapping ─────────────────────────────────────────

    #[tokio::test]
    async fn timeout_maps_to_diag_timeout() {
        let (mgr, _) = uds(vec![Err(ConnectionError::Timeout("test".into()))]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::Timeout));
    }

    #[tokio::test]
    async fn closed_maps_to_connection_closed() {
        let (mgr, _) = uds(vec![Err(ConnectionError::Closed)]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::ConnectionClosed(_)));
    }

    #[tokio::test]
    async fn send_failed_maps_to_send_failed() {
        let (mgr, _) = uds(vec![Err(ConnectionError::SendFailed("broken".into()))]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::SendFailed(_)));
    }

    #[tokio::test]
    async fn busy_repeat_request_maps_to_resource_error() {
        let (mgr, _) = uds(vec![Ok(UdsResponse::BusyRepeatRequest(0x0001))]);
        let err = mgr.read_data_by_identifier(0xF190).await.unwrap_err();
        assert!(matches!(err, DiagServiceError::ResourceError(_)));
    }

    // ── Initial state ───────────────────────────────────────────────────

    #[tokio::test]
    async fn initial_session_is_default() {
        let (mgr, _) = uds(vec![]);
        assert_eq!(mgr.current_session().await, DiagnosticSession::Default);
        assert!(!mgr.is_security_unlocked().await);
    }
}
