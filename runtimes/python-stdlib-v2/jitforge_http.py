import contextlib
import ipaddress
import json
import os
import socket
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request


_MAX_REQUESTS = 4
_MAX_RESPONSE_BYTES = 1024 * 1024
_request_count = 0
_network_depth = 0
_audit_installed = False


class Response:
    def __init__(self, url, status, content_type, body):
        self.url = url
        self.status = status
        self.content_type = content_type
        self.text = body

    def json(self):
        return json.loads(self.text)


def _load_json_file(path):
    with open(path, "r", encoding="utf-8") as source:
        return json.load(source)


def _manifest_capabilities():
    manifest = _load_json_file("/opt/jitforge/manifest.json")
    return manifest.get("http_capabilities", [])


def _canonical_url(raw_url, query=None):
    parsed = urllib.parse.urlsplit(raw_url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("jitforge_http only permits credential-free HTTPS URLs")
    if parsed.port not in (None, 443) or parsed.fragment:
        raise ValueError("jitforge_http only permits HTTPS port 443 without fragments")
    pairs = urllib.parse.parse_qsl(parsed.query, keep_blank_values=True)
    if query:
        for key, value in query.items():
            values = value if isinstance(value, (list, tuple)) else [value]
            for item in values:
                pairs.append((str(key), str(item)))
    pairs.sort()
    path = parsed.path or "/"
    return urllib.parse.urlunsplit(("https", parsed.hostname.lower(), path, urllib.parse.urlencode(pairs, doseq=True), ""))


def _matching_capability(url):
    parsed = urllib.parse.urlsplit(url)
    keys = {key for key, _ in urllib.parse.parse_qsl(parsed.query, keep_blank_values=True)}
    for grant in _manifest_capabilities():
        capability = grant.get("capability", {})
        if (
            capability.get("scheme") == "https"
            and capability.get("method") == "GET"
            and capability.get("port") == 443
            and capability.get("host", "").lower() == (parsed.hostname or "").lower()
            and parsed.path.startswith(capability.get("path_prefix", ""))
            and keys.issubset(set(capability.get("query_keys", [])))
        ):
            return capability
    raise PermissionError("HTTP request is outside the artifact's approved capabilities")


def _validate_public_dns(host):
    if host.lower() == "localhost":
        raise PermissionError("localhost is not a public HTTP destination")
    try:
        ipaddress.ip_address(host)
    except ValueError:
        pass
    else:
        raise PermissionError("IP-literal HTTP destinations are not allowed")
    addresses = {item[4][0] for item in socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)}
    if not addresses:
        raise OSError("HTTP destination did not resolve")
    for address in addresses:
        ip = ipaddress.ip_address(address)
        if not ip.is_global:
            raise PermissionError("HTTP destination resolved to a non-public address")


@contextlib.contextmanager
def _network_scope():
    global _network_depth
    _network_depth += 1
    try:
        yield
    finally:
        _network_depth -= 1


class _RedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        canonical = _canonical_url(new_url)
        _matching_capability(canonical)
        _validate_public_dns(urllib.parse.urlsplit(canonical).hostname)
        return super().redirect_request(request, fp, code, message, headers, canonical)


def _direct_get(url, timeout):
    host = urllib.parse.urlsplit(url).hostname
    with _network_scope():
        _validate_public_dns(host)
        opener = urllib.request.build_opener(
            _RedirectHandler(),
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
        )
        request = urllib.request.Request(
            url,
            headers={
                "Accept": "application/json,text/plain;q=0.9,*/*;q=0.2",
                "User-Agent": "JITForge/0.1 runtime",
            },
            method="GET",
        )
        try:
            response = opener.open(request, timeout=timeout)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read(_MAX_RESPONSE_BYTES + 1)
            if len(body) > _MAX_RESPONSE_BYTES:
                raise ValueError("HTTP response exceeded 1 MiB")
            content_type = response.headers.get_content_type()
            charset = response.headers.get_content_charset() or "utf-8"
            return Response(response.geturl(), response.status, content_type, body.decode(charset, errors="replace"))


def _fixture_get(url):
    raw = os.environ.get("JITFORGE_HTTP_FIXTURES", "[]")
    fixtures = json.loads(raw)
    for fixture in fixtures:
        if _canonical_url(fixture["request_url"]) == url:
            return Response(
                fixture.get("response_url", url),
                int(fixture["status"]),
                fixture.get("content_type", "application/octet-stream"),
                fixture.get("body", ""),
            )
    raise RuntimeError(f"no HTTP fixture matched {url}")


def get(url, query=None, timeout=10):
    global _request_count
    _request_count += 1
    if _request_count > _MAX_REQUESTS:
        raise RuntimeError("HTTP request budget exceeded")
    timeout = max(0.1, min(float(timeout), 10.0))
    canonical = _canonical_url(url, query)
    _matching_capability(canonical)
    mode = os.environ.get("JITFORGE_HTTP_MODE", "disabled")
    if mode == "fixture":
        return _fixture_get(canonical)
    if mode == "direct":
        return _direct_get(canonical, timeout)
    raise PermissionError("HTTP access is disabled for this invocation")


def _install_audit_hook():
    global _audit_installed
    if _audit_installed:
        return

    def audit(event, args):
        if event.startswith("socket.") and _network_depth <= 0:
            raise PermissionError("direct socket access is forbidden; use jitforge_http.get")
        if event in {"os.system", "os.exec", "subprocess.Popen"}:
            raise PermissionError("process execution is forbidden")

    sys.addaudithook(audit)
    _audit_installed = True
