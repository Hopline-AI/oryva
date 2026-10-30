"""Canned responses for the sarvam-lite fixture API, for live agent runs.

Binds a free port on 127.0.0.1, prints it on the first line of stdout, then serves until killed.
"""

import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PRONUNCIATIONS = {
    "p1": {"id": "p1", "word": "Sarvam", "pronunciation": "sur-vum"},
    "p2": {"id": "p2", "word": "Bulbul", "pronunciation": "bool-bool"},
}

ROUTES = {
    ("POST", "/translate"): {
        "request_id": "req_1",
        "translated_text": "सुप्रभात",
        "source_language_code": "en-IN",
    },
    ("POST", "/text-lid"): {"language_code": "bn-IN", "script_code": "Beng"},
    ("GET", "/pronunciations"): list(PRONUNCIATIONS.values()),
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _send(self, status, body=None):
        self.send_response(status)
        if body is None:
            self.end_headers()
            return
        data = json.dumps(body).encode()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _handle(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        if not self.headers.get("api-subscription-key"):
            return self._send(401, {"error": {"message": "missing api-subscription-key", "code": "unauthorized"}})
        path = self.path.split("?")[0]
        if (self.command, path) in ROUTES:
            return self._send(200, ROUTES[(self.command, path)])
        if path.startswith("/pronunciations/"):
            pid = path.rsplit("/", 1)[1]
            if pid not in PRONUNCIATIONS:
                return self._send(404, {"error": {"message": f"no pronunciation {pid}", "code": "not_found"}})
            if self.command == "GET":
                return self._send(200, PRONUNCIATIONS[pid])
            if self.command == "DELETE":
                return self._send(204)
        self._send(404, {"error": {"message": f"no route {self.command} {path}", "code": "not_found"}})

    do_GET = do_POST = do_DELETE = _handle


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
