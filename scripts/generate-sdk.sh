#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# generate-sdk.sh — Client SDK generation from OpenAPI spec (Wave 3, E3.1)
#
# Prerequisites:
#   - Running opensovd-native-server instance (default: http://localhost:8080)
#   - openapi-generator-cli (npm install @openapitools/openapi-generator-cli -g)
#     OR docker (fallback)
#
# Usage:
#   ./scripts/generate-sdk.sh [language] [server-url]
#
# Examples:
#   ./scripts/generate-sdk.sh python
#   ./scripts/generate-sdk.sh typescript-fetch http://localhost:9090
#   ./scripts/generate-sdk.sh rust
#
# Supported languages: python, typescript-fetch, rust, java, go, csharp
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

LANG="${1:-python}"
SERVER_URL="${2:-http://localhost:8080}"
OPENAPI_URL="${SERVER_URL}/sovd/v1/openapi.json"
OUTPUT_DIR="generated-sdk/${LANG}"

echo "=== OpenSOVD Client SDK Generator ==="
echo "Language:   ${LANG}"
echo "Server:     ${SERVER_URL}"
echo "OpenAPI:    ${OPENAPI_URL}"
echo "Output:     ${OUTPUT_DIR}"
echo ""

# 1. Fetch OpenAPI spec
echo "→ Fetching OpenAPI spec..."
SPEC_FILE=$(mktemp /tmp/opensovd-openapi-XXXXXX.json)
trap 'rm -f "${SPEC_FILE}"' EXIT

if ! curl -sf "${OPENAPI_URL}" -o "${SPEC_FILE}"; then
    echo "ERROR: Failed to fetch OpenAPI spec from ${OPENAPI_URL}"
    echo "       Is the server running? Try: cargo run -p opensovd-native-server"
    exit 1
fi

echo "  ✓ Spec fetched ($(wc -c < "${SPEC_FILE}" | tr -d ' ') bytes)"

# 2. Generate SDK
mkdir -p "${OUTPUT_DIR}"

if command -v openapi-generator-cli &>/dev/null; then
    echo "→ Generating SDK with openapi-generator-cli..."
    openapi-generator-cli generate \
        -i "${SPEC_FILE}" \
        -g "${LANG}" \
        -o "${OUTPUT_DIR}" \
        --additional-properties=packageName=opensovd_client,projectName=opensovd-client
elif command -v docker &>/dev/null; then
    echo "→ Generating SDK with Docker (openapitools/openapi-generator-cli)..."
    docker run --rm \
        -v "${PWD}/${OUTPUT_DIR}:/output" \
        -v "${SPEC_FILE}:/spec.json:ro" \
        openapitools/openapi-generator-cli generate \
        -i /spec.json \
        -g "${LANG}" \
        -o /output \
        --additional-properties=packageName=opensovd_client,projectName=opensovd-client
else
    echo "ERROR: Neither openapi-generator-cli nor docker found."
    echo "       Install: npm install @openapitools/openapi-generator-cli -g"
    echo "       Or use Docker: docker pull openapitools/openapi-generator-cli"
    exit 1
fi

echo ""
echo "=== SDK generated successfully ==="
echo "Output directory: ${OUTPUT_DIR}"
echo ""
echo "Next steps:"
case "${LANG}" in
    python)
        echo "  cd ${OUTPUT_DIR} && pip install -e ."
        ;;
    typescript-fetch|typescript-axios)
        echo "  cd ${OUTPUT_DIR} && npm install && npm run build"
        ;;
    rust)
        echo "  cd ${OUTPUT_DIR} && cargo build"
        ;;
    java)
        echo "  cd ${OUTPUT_DIR} && mvn package"
        ;;
    go)
        echo "  cd ${OUTPUT_DIR} && go build ./..."
        ;;
    *)
        echo "  See ${OUTPUT_DIR}/README.md for build instructions"
        ;;
esac
