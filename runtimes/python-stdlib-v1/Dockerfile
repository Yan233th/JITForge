FROM python:3.13-alpine

RUN addgroup -g 65532 jitforge \
    && adduser -D -H -u 65532 -G jitforge jitforge \
    && mkdir -p /opt/jitforge \
    && chown -R 65532:65532 /opt/jitforge

WORKDIR /tmp
USER 65532:65532
