// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// k6 Load Test — OpenSOVD-native-server (T2.1)
//
// Usage:
//   k6 run tests/load/k6-sovd.js
//   k6 run --vus 50 --duration 60s tests/load/k6-sovd.js
//   k6 run --env BASE_URL=https://localhost:8443 tests/load/k6-sovd.js
//
// Scenarios:
//   1. smoke     — 1 VU, 10s (sanity check)
//   2. load      — 20 VUs, 60s (sustained load)
//   3. stress    — 50 VUs, 30s ramp + 60s peak + 30s cooldown
//   4. spike     — 5→100→5 VUs over 40s
//
// Thresholds:
//   - p(95) response time < 500ms
//   - Error rate < 1%
//   - p(99) response time < 1000ms
// ─────────────────────────────────────────────────────────────────────────────

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend } from "k6/metrics";

// ── Configuration ────────────────────────────────────────────────────────────

const BASE_URL = __ENV.BASE_URL || "http://localhost:8080";
const SOVD_BASE = `${BASE_URL}/sovd/v1`;

// Custom metrics
const errorRate = new Rate("sovd_errors");
const discoveryDuration = new Trend("sovd_discovery_duration", true);
const faultDuration = new Trend("sovd_fault_duration", true);
const dataDuration = new Trend("sovd_data_duration", true);
const healthDuration = new Trend("sovd_health_duration", true);

// ── Thresholds ───────────────────────────────────────────────────────────────

export const options = {
  scenarios: {
    smoke: {
      executor: "constant-vus",
      vus: 1,
      duration: "10s",
      tags: { scenario: "smoke" },
    },
    load: {
      executor: "constant-vus",
      vus: 20,
      duration: "60s",
      startTime: "15s",
      tags: { scenario: "load" },
    },
    stress: {
      executor: "ramping-vus",
      startVUs: 0,
      stages: [
        { duration: "30s", target: 50 },
        { duration: "60s", target: 50 },
        { duration: "30s", target: 0 },
      ],
      startTime: "80s",
      tags: { scenario: "stress" },
    },
    spike: {
      executor: "ramping-vus",
      startVUs: 5,
      stages: [
        { duration: "10s", target: 100 },
        { duration: "20s", target: 100 },
        { duration: "10s", target: 5 },
      ],
      startTime: "200s",
      tags: { scenario: "spike" },
    },
  },
  thresholds: {
    http_req_duration: ["p(95)<500", "p(99)<1000"],
    sovd_errors: ["rate<0.01"],
    sovd_discovery_duration: ["p(95)<300"],
    sovd_fault_duration: ["p(95)<300"],
    sovd_data_duration: ["p(95)<300"],
    sovd_health_duration: ["p(95)<200"],
  },
};

// ── Helper ───────────────────────────────────────────────────────────────────

function sovdGet(path, metricTrend) {
  const res = http.get(`${SOVD_BASE}${path}`, {
    headers: { Accept: "application/json" },
    tags: { endpoint: path },
  });

  if (metricTrend) {
    metricTrend.add(res.timings.duration);
  }

  const ok = check(res, {
    "status is 2xx": (r) => r.status >= 200 && r.status < 300,
    "has body": (r) => r.body && r.body.length > 0,
  });

  errorRate.add(!ok);
  return res;
}

// ── Default function (VU iteration) ──────────────────────────────────────────

export default function () {
  // 1. Discovery — list components
  sovdGet("/components", discoveryDuration);

  // 2. Health endpoint
  sovdGet("/health", healthDuration);

  // 3. List faults for a component (may 404 if no mock components)
  const compRes = http.get(`${SOVD_BASE}/components`, {
    headers: { Accept: "application/json" },
  });
  if (compRes.status === 200) {
    try {
      const body = JSON.parse(compRes.body);
      const components = body.value || [];
      if (components.length > 0) {
        const cid = components[0].id;

        // Faults
        sovdGet(`/components/${cid}/faults`, faultDuration);

        // Data catalog
        sovdGet(`/components/${cid}/data`, dataDuration);
      }
    } catch (_) {
      // Ignore parse errors
    }
  }

  // 4. Audit trail
  sovdGet("/audit?limit=10", null);

  // 5. Feature flags
  http.get(`${BASE_URL}/x-admin/features`, {
    headers: { Accept: "application/json" },
  });

  // 6. Liveness probe
  http.get(`${BASE_URL}/healthz`);

  // Think time — simulate realistic user behavior
  sleep(Math.random() * 2 + 0.5);
}

// ── Setup / Teardown ─────────────────────────────────────────────────────────

export function setup() {
  // Verify server is reachable
  const res = http.get(`${BASE_URL}/healthz`);
  check(res, {
    "server is alive": (r) => r.status === 200,
  });
  if (res.status !== 200) {
    throw new Error(`Server not reachable at ${BASE_URL}`);
  }
  console.log(`Load test targeting: ${BASE_URL}`);
}

export function teardown() {
  console.log("Load test complete");
}
