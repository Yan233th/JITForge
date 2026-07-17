# syntax=docker/dockerfile:1.7

# Rust is compiled on the host. Container images only package release binaries.

FROM docker:29-cli AS docker-cli

FROM debian:13-slim AS server
COPY target/release/jit-server /usr/local/bin/jit-server
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/jit-server"]

FROM debian:13-slim AS worker
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY target/release/jit-worker /usr/local/bin/jit-worker
ENTRYPOINT ["/usr/local/bin/jit-worker"]
