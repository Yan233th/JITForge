# syntax=docker/dockerfile:1.7

# Rust is compiled on the host. This file only packages existing GNU/glibc
# binaries when containerized control-plane services are explicitly wanted.
ARG JITFORGE_BUILD_PROFILE=release

FROM docker:29-cli AS docker-cli

FROM debian:13-slim AS server
ARG JITFORGE_BUILD_PROFILE
COPY target/${JITFORGE_BUILD_PROFILE}/jit-server /usr/local/bin/jit-server
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/jit-server"]

FROM debian:13-slim AS worker
ARG JITFORGE_BUILD_PROFILE
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY target/${JITFORGE_BUILD_PROFILE}/jit-worker /usr/local/bin/jit-worker
ENTRYPOINT ["/usr/local/bin/jit-worker"]
