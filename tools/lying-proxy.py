#!/usr/bin/env python3
"""A proxy in front of a Subsonic server that states transcoded songs longer than they are.

    tools/lying-proxy.py [port=4534] [upstream=http://localhost:4533]

Everything passes through as it is, except a transcoded stream (`/rest/stream` with a `format` other than
raw): its answer claims a Content-Length a quarter longer than the bytes that follow, then the connection
closes - what a server that answers `estimateContentLength=true` with an estimate does whenever the real
transcode comes out smaller. A range asked from past the real end is answered 416, as such a server does;
a range inside it gets those bytes. Other songs are sent at about 4 MB/s, so downloads over it take long
enough to be seen running side by side. The e2e checks' local mode (NORI_E2E_SERVER=local) puts the app behind
this, so the player meets a song whose stated length it can never reach (the 416 bug).

Hang mode stands in for octo-fiesta asked for a provider's song it cannot fetch (its token gone): a
stream or download of a chosen song id is never answered ("headers") or answered with its headers and
then not a byte ("body"), until the client gives up and closes the connection. Choose them at start with
NORI_HANG="id1,id2" (and NORI_HANG_MODE=headers|body), or at any time over HTTP:

    curl 'localhost:4534/_hang?ids=id1,id2&mode=body'   # these hang from now on (none: nothing hangs)
    curl 'localhost:4534/_hang'                          # which hang, how many requests hang now, how
                                                         # many hung in all and how many the client closed
"""
import http.client
import json
import os
import select
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 4534
UP = urllib.parse.urlsplit(sys.argv[2] if len(sys.argv) > 2 else "http://localhost:4533")
# What each transcode really came to, by song, format and bit rate: a range past it is refused.
real = {}
lock = threading.Lock()
# Hang mode: the song ids that hang, how, and the requests that hang now, hung in all, and were closed.
hang = {"ids": set(filter(None, os.environ.get("NORI_HANG", "").split(","))), "mode": os.environ.get("NORI_HANG_MODE", "headers")}
hanging = {"now": 0, "all": 0, "closed": 0}


def hangs(path):
    u = urllib.parse.urlsplit(path)
    if not u.path.startswith(("/rest/stream", "/rest/download")):
        return False
    with lock:
        return urllib.parse.parse_qs(u.query).get("id", [""])[0] in hang["ids"]


def transcoded(path):
    u = urllib.parse.urlsplit(path)
    if not u.path.startswith("/rest/stream"):
        return None
    q = urllib.parse.parse_qs(u.query)
    fmt = q.get("format", [""])[0]
    if fmt in ("", "raw"):
        return None
    return (q.get("id", [""])[0], fmt, q.get("maxBitRate", [""])[0])


class Proxy(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def upstream(self, method, body=None, drop_range=False):
        c = http.client.HTTPConnection(UP.hostname, UP.port or 80, timeout=120)
        headers = {k: v for k, v in self.headers.items() if k.lower() not in ("host", "connection") and not (drop_range and k.lower() == "range")}
        c.request(method, self.path, body=body, headers=headers)
        return c, c.getresponse()

    def control(self):
        """/_hang: sets the songs that hang (`ids`, `mode`) when asked, and says how it stands."""
        q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query, keep_blank_values=True)
        with lock:
            if "ids" in q:
                hang["ids"] = set(filter(None, q["ids"][0].split(",")))
            if "mode" in q:
                hang["mode"] = q["mode"][0]
            said = json.dumps({"ids": sorted(hang["ids"]), "mode": hang["mode"], **hanging}).encode() + b"\n"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(said)))
        self.end_headers()
        self.wfile.write(said)

    def hang(self):
        """Never answers (or answers the headers and sends nothing), until the client closes the connection."""
        with lock:
            mode = hang["mode"]
            hanging["now"] += 1
            hanging["all"] += 1
        try:
            if mode == "body":
                self.send_response(200)
                self.send_header("Content-Type", "audio/mpeg")
                self.send_header("Content-Length", "8000000")
                self.end_headers()
                self.wfile.flush()
            sock = self.connection
            while True:
                readable, _, _ = select.select([sock], [], [], 1.0)
                if readable and not sock.recv(1, 2):  # MSG_PEEK: an empty read is the client gone
                    break
            with lock:
                hanging["closed"] += 1
        except OSError:
            with lock:
                hanging["closed"] += 1
        finally:
            with lock:
                hanging["now"] -= 1
        self.close_connection = True

    def relay(self, method):
        if urllib.parse.urlsplit(self.path).path == "/_hang":
            return self.control()
        if hangs(self.path):
            return self.hang()
        body = None
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            body = self.rfile.read(n)
        key = transcoded(self.path) if method == "GET" else None
        if key is None:
            c, r = self.upstream(method, body)
            data = r.read()
            self.send_response(r.status)
            for k, v in r.getheaders():
                if k.lower() not in ("transfer-encoding", "connection", "content-length"):
                    self.send_header(k, v)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            if method != "HEAD":
                # Songs come at about a real connection's pace (4 MB/s), not a loopback's: over a LAN a
                # generated album downloads before anything can see two of its songs running at once.
                paced = urllib.parse.urlsplit(self.path).path.startswith(("/rest/stream", "/rest/download"))
                try:
                    for i in range(0, len(data), 65536):
                        self.wfile.write(data[i:i + 65536])
                        if paced:
                            time.sleep(0.016)
                except (BrokenPipeError, ConnectionResetError):
                    self.close_connection = True
            c.close()
            return
        start = 0
        rng = self.headers.get("Range", "")
        if rng.startswith("bytes="):
            start = int(rng[6:].split("-")[0] or 0)
        with lock:
            known = real.get(key)
        if start > 0 and known is not None and start >= len(known):
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{len(known)}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if known is None:
            c, r = self.upstream("GET", drop_range=True)
            try:
                data = r.read()
            except http.client.IncompleteRead as short:
                # Navidrome itself states the estimate on a transcode it has not cached yet and closes
                # short of it: what came is the whole transcode.
                data = short.partial
            c.close()
            if r.status != 200:
                self.send_response(r.status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            with lock:
                real[key] = data
            known = data
            if start >= len(known) and start > 0:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{len(known)}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
        claimed = len(known) * 5 // 4 + 1000
        if start > 0:
            part = known[start:]
            self.send_response(206)
            self.send_header("Content-Type", "audio/ogg")
            self.send_header("Content-Range", f"bytes {start}-{len(known) - 1}/{claimed}")
            self.send_header("Content-Length", str(len(part)))
            self.end_headers()
            self.wfile.write(part)
            return
        # The lie: a length the bytes never reach, then the connection goes.
        self.send_response(200)
        self.send_header("Content-Type", "audio/ogg")
        self.send_header("Content-Length", str(claimed))
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(known)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        self.close_connection = True

    def do_GET(self):
        self.relay("GET")

    def do_HEAD(self):
        self.relay("HEAD")

    def do_POST(self):
        self.relay("POST")


if __name__ == "__main__":
    ThreadingHTTPServer.daemon_threads = True
    ThreadingHTTPServer(("0.0.0.0", PORT), Proxy).serve_forever()
