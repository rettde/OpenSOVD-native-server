// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostic Log Buffer — ring buffer for diagnostic log entries (SOVD §7.10)
// Captures diagnostic events for later retrieval via REST API.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::VecDeque;
use std::sync::Mutex;

use native_interfaces::sovd::{SovdLogEntry, SovdLogLevel};

const DEFAULT_MAX_ENTRIES: usize = 1000;

/// Thread-safe diagnostic log buffer with bounded capacity
pub struct DiagLog {
    entries: Mutex<VecDeque<SovdLogEntry>>,
    max_entries: usize,
}

impl DiagLog {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(DEFAULT_MAX_ENTRIES)),
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(max_entries)),
            max_entries,
        }
    }

    /// Append a log entry. Oldest entries are evicted when capacity is reached.
    pub fn append(
        &self,
        source: &str,
        level: SovdLogLevel,
        message: &str,
        data: Option<serde_json::Value>,
    ) {
        let entry = SovdLogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level,
            source: source.to_owned(),
            message: message.to_owned(),
            data,
        };
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Get all log entries (optionally filtered by component source)
    pub fn get_entries(&self, source_filter: Option<&str>) -> Vec<SovdLogEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match source_filter {
            Some(src) => entries
                .iter()
                .filter(|e| e.source == src)
                .cloned()
                .collect(),
            None => entries.iter().cloned().collect(),
        }
    }

    /// Get recent N entries
    pub fn recent(&self, count: usize) -> Vec<SovdLogEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Default for DiagLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_retrieve() {
        let log = DiagLog::new();
        log.append("hpc", SovdLogLevel::Info, "Connected", None);
        log.append("brake", SovdLogLevel::Warning, "Timeout", None);
        assert_eq!(log.len(), 2);
        let all = log.get_entries(None);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn filter_by_source() {
        let log = DiagLog::new();
        log.append("hpc", SovdLogLevel::Info, "msg1", None);
        log.append("brake", SovdLogLevel::Info, "msg2", None);
        log.append("hpc", SovdLogLevel::Error, "msg3", None);
        let hpc = log.get_entries(Some("hpc"));
        assert_eq!(hpc.len(), 2);
        let brake = log.get_entries(Some("brake"));
        assert_eq!(brake.len(), 1);
    }

    #[test]
    fn evicts_oldest_when_full() {
        let log = DiagLog::with_capacity(3);
        log.append("a", SovdLogLevel::Info, "1", None);
        log.append("a", SovdLogLevel::Info, "2", None);
        log.append("a", SovdLogLevel::Info, "3", None);
        log.append("a", SovdLogLevel::Info, "4", None);
        assert_eq!(log.len(), 3);
        let entries = log.get_entries(None);
        assert_eq!(entries[0].message, "2");
        assert_eq!(entries[2].message, "4");
    }

    #[test]
    fn recent_returns_last_n() {
        let log = DiagLog::new();
        for i in 0..10 {
            log.append("x", SovdLogLevel::Info, &format!("msg{i}"), None);
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].message, "msg7");
        assert_eq!(recent[2].message, "msg9");
    }

    #[test]
    fn clear_empties_log() {
        let log = DiagLog::new();
        log.append("x", SovdLogLevel::Info, "test", None);
        assert!(!log.is_empty());
        log.clear();
        assert!(log.is_empty());
    }
}
