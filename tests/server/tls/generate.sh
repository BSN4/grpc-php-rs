#!/usr/bin/env bash
# Regenerates the test-only PKI in this directory.
#
# These keys are DELIBERATELY checked in and secure nothing — they exist so
# the test server can offer TLS (tonic requires a CA-signed chain) and so the
# dockerized official ext-grpc can verify the hostname during parity dual-runs
# (hence the host.docker.internal SAN). 100-year validity so the fixture never
# becomes a surprise CI failure. Re-run only to change the SAN list.
set -euo pipefail
cd "$(dirname "$0")"

openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out ca.key
openssl req -x509 -new -key ca.key -days 36500 \
    -subj "/CN=grpc-php-rs-test-ca" \
    -addext "basicConstraints=critical,CA:TRUE" -out ca.crt

openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out server.key
openssl req -new -key server.key -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,DNS:host.docker.internal,IP:127.0.0.1,IP:::1" \
    -out server.csr

openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
    -days 36500 -copy_extensions copy -out server.crt
rm server.csr ca.srl

echo "Regenerated. The server embeds these via include_str! — rebuild it:"
echo "  cargo build --release --manifest-path tests/server/Cargo.toml"
