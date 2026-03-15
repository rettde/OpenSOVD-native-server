// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// OTA Firmware Flash Orchestrator
// Full UDS flash workflow: 0x10→0x27→0x34→0x36→0x37→0x11
// Follows the OpenSOVD design "Flash Service App" pattern
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use tracing::info;

use native_comm_uds::UdsManager;
use native_interfaces::{DiagServiceError, DiagnosticSession};

/// Orchestrates the full OTA firmware flash sequence over UDS
pub struct OtaFlashOrchestrator;

impl OtaFlashOrchestrator {
    /// Full OTA firmware flash workflow:
    ///  1. Switch to Programming Session (SID 0x10, sub=0x02)
    ///  2. Security Access (SID 0x27) — request seed + send key
    ///  3. Request Download (SID 0x34) — negotiate block size
    ///  4. Transfer Data (SID 0x36) — send firmware in chunks
    ///  5. Request Transfer Exit (SID 0x37)
    ///  6. ECU Reset (SID 0x11) — hard reset to activate new firmware
    #[tracing::instrument(skip(uds, firmware_data), fields(
        component = %component_id,
        size = firmware_data.len(),
        addr = %format!("0x{memory_address:08X}"),
    ))]
    pub async fn flash(
        uds: &Arc<UdsManager>,
        component_id: &str,
        firmware_data: &[u8],
        memory_address: u32,
    ) -> Result<FlashResult, DiagServiceError> {
        let total_size = firmware_data.len();

        info!("Starting OTA flash: {} bytes", total_size);

        // Step 1: Switch to programming session
        uds.diagnostic_session_control(DiagnosticSession::Programming)
            .await?;
        info!("[Flash] Programming session active");

        // Step 2: Security Access (seed/key exchange)
        let seed = uds.security_access_request_seed(0x01).await?;
        // Derive key from seed (simple XOR-based derivation for demo)
        let key: Vec<u8> = seed.iter().map(|b| b ^ 0xFF).collect();
        uds.security_access_send_key(0x02, &key).await?;
        info!("[Flash] Security access granted");

        // Step 3: Request Download — get max block size
        let max_block_size = uds
            .request_download(memory_address, total_size as u32, 0x00, 0x00)
            .await?;

        // Use max_block_size minus 2 bytes overhead (block counter + SID)
        let chunk_size = (max_block_size as usize).saturating_sub(2).max(256);
        let total_blocks = total_size.div_ceil(chunk_size);
        info!("[Flash] Download negotiated: block_size={chunk_size}, chunks={total_blocks}");

        // Step 4: Transfer Data in chunks
        let mut block_seq: u8 = 1;
        let mut offset = 0usize;

        while offset < total_size {
            let end = (offset + chunk_size).min(total_size);
            let chunk = &firmware_data[offset..end];

            uds.transfer_data(block_seq, chunk).await?;

            let progress = ((end as f64 / total_size as f64) * 100.0) as u8;
            info!("[Flash] Block {block_seq} transferred ({end}/{total_size} bytes, {progress}%)");

            block_seq = block_seq.wrapping_add(1);
            offset = end;
        }

        // Step 5: Request Transfer Exit
        uds.request_transfer_exit().await?;
        info!("[Flash] Transfer complete");

        // Step 6: ECU Reset (hard reset)
        uds.ecu_reset(0x01).await?;
        info!("[Flash] ECU reset triggered — firmware activation pending");

        Ok(FlashResult {
            component_id: component_id.to_owned(),
            bytes_transferred: total_size,
            blocks_transferred: total_blocks,
            memory_address: format!("0x{memory_address:08X}"),
        })
    }
}

/// Result of a successful firmware flash
#[derive(Debug, Clone, serde::Serialize)]
pub struct FlashResult {
    pub component_id: String,
    pub bytes_transferred: usize,
    pub blocks_transferred: usize,
    pub memory_address: String,
}
