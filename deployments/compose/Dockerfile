# syntax=docker/dockerfile:1.7

# Rust is compiled on the host. This file only packages existing GNU/glibc
# binaries when containerized control-plane services are explicitly wanted.
FROM debian:13-slim AS server
COPY target/release/jit-server /usr/local/bin/jit-server
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/jit-server"]

FROM docker:29-cli AS docker-cli

FROM debian:13-slim AS worker
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY target/release/jit-worker /usr/local/bin/jit-worker
ENTRYPOINT ["/usr/local/bin/jit-worker"]
