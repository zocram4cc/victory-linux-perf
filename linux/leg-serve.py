#!/usr/bin/env python3
"""leg-serve.py <captures-root> <address> <port>: serves the legs' index and
its pictures on the local network, and nothing else of the folder (no logs,
no listings): GET / or /index.html, and /<leg>/<name>.jpg."""
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

root = Path(sys.argv[1]).resolve()
ok = re.compile(r'^/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+\.(jpg|mp4)$')


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split('?', 1)[0]
        if path in ('/', '/index.html'):
            file, kind = root / 'index.html', 'text/html; charset=utf-8'
        elif ok.match(path) and '..' not in path:
            file, kind = root / path.lstrip('/'), ('video/mp4' if path.endswith('.mp4') else 'image/jpeg')
        else:
            self.send_error(404); return
        try:
            data = file.read_bytes()
        except OSError:
            self.send_error(404); return
        self.send_response(200)
        self.send_header('Content-Type', kind)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


ThreadingHTTPServer((sys.argv[2], int(sys.argv[3])), Handler).serve_forever()
