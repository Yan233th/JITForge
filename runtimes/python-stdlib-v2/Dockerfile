FROM python:3.13-alpine

RUN addgroup -g 65532 jitforge \
    && adduser -D -H -u 65532 -G jitforge jitforge \
    && mkdir -p /opt/jitforge /opt/jitforge-runtime \
    && chown -R 65532:65532 /opt/jitforge /opt/jitforge-runtime

COPY entrypoint.py /opt/jitforge-runtime/entrypoint.py
COPY jitforge_http.py /usr/local/lib/python3.13/jitforge_http.py

WORKDIR /tmp
USER 65532:65532
ENTRYPOINT ["python3", "-I", "-S", "-B", "/opt/jitforge-runtime/entrypoint.py", "/opt/jitforge/tool.py"]
