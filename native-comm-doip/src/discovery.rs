// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// DoIP vehicle discovery via UDP broadcast (VIR/VAM)
// Follows cda-comm-doip vir_vam.rs patterns
// Uses doip-definitions builder + ProtocolVersion
// ─────────────────────────────────────────────────────────────────────────────

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, info};

use doip_definitions::builder::DoipMessageBuilder;
use doip_definitions::header::ProtocolVersion;
use doip_definitions::payload::{DoipPayload, VehicleIdentificationRequest};

use native_interfaces::ConnectionError;

/// Discovered DoIP entity from a Vehicle Announcement Message
#[derive(Debug, Clone)]
pub struct DiscoveredEntity {
    pub vin: String,
    pub logical_address: u16,
    pub eid: [u8; 6],
    pub gid: [u8; 6],
    pub source_addr: SocketAddr,
}

/// Discover DoIP entities on the network via UDP broadcast (ISO 13400-2 VIR/VAM)
#[tracing::instrument(skip_all, fields(bind_addr = %bind_addr, timeout_ms))]
pub async fn discover_vehicles(
    bind_addr: &str,
    timeout_ms: Option<u64>,
) -> Result<Vec<DiscoveredEntity>, ConnectionError> {
    let timeout_ms = timeout_ms.unwrap_or(3000);

    let socket = UdpSocket::bind(bind_addr)
        .await
        .map_err(|e| ConnectionError::ConnectionFailed(format!("UDP bind: {e}")))?;

    socket
        .set_broadcast(true)
        .map_err(|e| ConnectionError::ConnectionFailed(format!("Set broadcast: {e}")))?;

    // Build Vehicle Identification Request (VIR) using the builder
    let vir_message = DoipMessageBuilder::new()
        .protocol_version(ProtocolVersion::Iso13400_2019)
        .payload(DoipPayload::VehicleIdentificationRequest(
            VehicleIdentificationRequest {},
        ))
        .build();

    // Serialize message to bytes: header (8 bytes) + payload (0 bytes for VIR)
    let header_bytes: [u8; 8] = vir_message.header.into();
    let broadcast_addr: SocketAddr = "255.255.255.255:13400"
        .parse()
        .map_err(|e| ConnectionError::ConnectionFailed(format!("Broadcast addr: {e}")))?;

    socket
        .send_to(&header_bytes, broadcast_addr)
        .await
        .map_err(|e| ConnectionError::SendFailed(format!("VIR send: {e}")))?;

    info!("Sent Vehicle Identification Request (VIR)");

    let mut entities = Vec::new();
    let mut buf = [0u8; 4096];

    let deadline = Duration::from_millis(timeout_ms);
    let start = tokio::time::Instant::now();

    loop {
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }

        match timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, addr))) => {
                debug!(len, %addr, "Received UDP response");
                if let Some(entity) = parse_vam(&buf[..len], addr) {
                    info!(vin = %entity.vin, addr = %format!("0x{:04X}", entity.logical_address), "Discovered DoIP entity");
                    entities.push(entity);
                }
            }
            Ok(Err(e)) => {
                debug!("UDP recv error: {e}");
                break;
            }
            Err(_) => break, // timeout
        }
    }

    info!(count = entities.len(), "Vehicle discovery complete");
    Ok(entities)
}

/// Parse a Vehicle Announcement Message (VAM) from raw bytes
fn parse_vam(data: &[u8], source_addr: SocketAddr) -> Option<DiscoveredEntity> {
    // Minimum DoIP header is 8 bytes, VAM payload is 32+ bytes
    if data.len() < 40 {
        return None;
    }

    // Check payload type at offset 2-3 (big-endian)
    let payload_type = u16::from_be_bytes([data[2], data[3]]);
    if payload_type != 0x0004 {
        // Not a VAM
        return None;
    }

    // Parse VAM fields after 8-byte header
    let payload = &data[8..];
    if payload.len() < 32 {
        return None;
    }

    let vin = String::from_utf8_lossy(&payload[0..17])
        .trim_end_matches('\0')
        .to_owned();
    let logical_address = u16::from_be_bytes([payload[17], payload[18]]);

    let mut eid = [0u8; 6];
    eid.copy_from_slice(&payload[19..25]);
    let mut gid = [0u8; 6];
    gid.copy_from_slice(&payload[25..31]);

    Some(DiscoveredEntity {
        vin,
        logical_address,
        eid,
        gid,
        source_addr,
    })
}
