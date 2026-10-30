"""A stub of fixtures/specs/ledger.yaml for SDK conformance checks in every language.

Binds a free port on 127.0.0.1, prints it on the first line of stdout, then serves until killed.
Stdlib only, so every language's smoke test can run it.

Credentials: bearer `test-key`, or a token from POST /oauth/token (client `test-client`,
secret `test-secret`).

Test hooks:
- `x-stub-fail: <status>:<n>` with `x-stub-request: <id>` fails the first <n> attempts carrying
  that request id with <status> (and `Retry-After: 0`), then serves normally.
- GET /_stub/requests lists every request received: method, path, query, selected headers and
  the raw body (latin-1 decoded). POST /_stub/reset clears it and the failure counters.

Deliberate quirks for forward-compatibility checks: invoice `inv_5` has status `uncollectible`
and reminder_level 3 (neither is in the spec), and the event stream ends with an event type the
spec doesn't define.
"""

import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

CUSTOMERS = [
    {"id": f"cus_{i}", "name": f"Customer {i}", "email": f"c{i}@example.com", "metadata": {},
     "created_at": f"2026-10-0{i}T00:00:00Z"}
    for i in range(1, 6)
]
INVOICES = [
    {"id": f"inv_{i}", "customer_id": "cus_1", "status": s, "currency": "usd",
     "line_items": [{"description": "Consulting", "amount": 1000 * i, "quantity": 1}],
     "total": 1000 * i, "reminder_level": r, "due_date": None}
    for i, s, r in [(1, "draft", 0), (2, "open", 1), (3, "paid", 0), (4, "void", 2), (5, "uncollectible", 3)]
]
PAYMENTS = [
    {"id": f"pay_{i}", "invoice_id": "inv_3", "amount": 500, "status": "succeeded", "failure_reason": None}
    for i in range(1, 7)
]
EVENTS = [
    {"type": "invoice.paid", "invoice": INVOICES[2]},
    {"type": "payment.failed", "payment": {**PAYMENTS[0], "status": "failed", "failure_reason": "card_declined"}},
    {"type": "customer.created", "customer": CUSTOMERS[0]},
]
PDF = b"%PDF-1.4\n% oryva stub invoice\n%%EOF\n"
RECORDED_HEADERS = ["authorization", "idempotency-key", "user-agent", "content-type", "x-custom", "accept"]

requests_seen = []
fail_counts = {}
tokens_issued = []


def error(code, message, param=None):
    return {"error": {"code": code, "message": message, "param": param}}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send_json(self, status, body, headers=None):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("x-request-id", f"req_{len(requests_seen)}")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def send_empty(self, status):
        self.send_response(status)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def handle_any(self):
        url = urlsplit(self.path)
        path, query = url.path, parse_qs(url.query)
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))

        if path == "/_stub/requests":
            return self.send_json(200, requests_seen)
        if path == "/_stub/reset":
            requests_seen.clear()
            fail_counts.clear()
            tokens_issued.clear()
            return self.send_empty(204)

        requests_seen.append({
            "method": self.command, "path": path, "query": {k: v[0] for k, v in query.items()},
            "headers": {h: self.headers[h] for h in RECORDED_HEADERS if h in self.headers},
            "body": body.decode("latin-1"),
        })

        if path == "/oauth/token" and self.command == "POST":
            form = {k: v[0] for k, v in parse_qs(body.decode()).items()}
            if (form.get("grant_type"), form.get("client_id"), form.get("client_secret")) != (
                    "client_credentials", "test-client", "test-secret"):
                return self.send_json(401, {"error": "invalid_client"})
            token = f"tok_{len(tokens_issued) + 1}"
            tokens_issued.append(token)
            return self.send_json(200, {"access_token": token, "token_type": "bearer", "expires_in": 3600})

        auth = self.headers.get("authorization", "")
        if auth not in ["Bearer test-key"] + [f"Bearer {t}" for t in tokens_issued]:
            return self.send_json(401, error("unauthorized", "missing or invalid credentials"))

        fail = self.headers.get("x-stub-fail")
        if fail:
            status, times = fail.split(":")
            key = self.headers.get("x-stub-request", "default")
            if fail_counts.get(key, 0) < int(times):
                fail_counts[key] = fail_counts.get(key, 0) + 1
                return self.send_json(int(status), error("injected", f"injected {status}"), {"Retry-After": "0"})

        route = (self.command, path)
        parts = path.strip("/").split("/")

        if route == ("GET", "/customers"):
            limit = int(query.get("limit", ["20"])[0])
            start = 0
            if "starting_after" in query:
                start = next(i for i, c in enumerate(CUSTOMERS) if c["id"] == query["starting_after"][0]) + 1
            page = CUSTOMERS[start:start + limit]
            more = start + limit < len(CUSTOMERS)
            return self.send_json(200, {"data": page, "next_cursor": page[-1]["id"] if more and page else None})
        if route == ("POST", "/customers"):
            data = json.loads(body or b"{}")
            if "name" not in data:
                return self.send_json(400, error("missing_field", "name is required", "name"))
            return self.send_json(201, {"id": "cus_new", "metadata": {}, "created_at": "2026-10-30T00:00:00Z",
                                        "email": None, **data})
        if route == ("GET", "/customers/search"):
            q = query.get("q", [""])[0].lower()
            return self.send_json(200, [c for c in CUSTOMERS if q in c["name"].lower()])
        if parts[0] == "customers" and len(parts) == 2:
            customer = next((c for c in CUSTOMERS if c["id"] == parts[1]), None)
            if customer is None:
                return self.send_json(404, error("not_found", f"no customer {parts[1]}"))
            if self.command == "GET":
                return self.send_json(200, customer)
            if self.command == "PATCH":
                return self.send_json(200, {**customer, **json.loads(body or b"{}")})
            if self.command == "DELETE":
                return self.send_empty(204)

        if route == ("GET", "/invoices"):
            limit, offset = int(query.get("limit", ["20"])[0]), int(query.get("offset", ["0"])[0])
            return self.send_json(200, {"data": INVOICES[offset:offset + limit], "total": len(INVOICES)})
        if route == ("POST", "/invoices"):
            data = json.loads(body or b"{}")
            total = sum(li["amount"] * li.get("quantity", 1) for li in data.get("line_items", []))
            return self.send_json(201, {"id": "inv_new", "status": "draft", "currency": data.get("currency", "usd"),
                                        "total": total, "reminder_level": 0, "due_date": None, **data})
        if parts[0] == "invoices" and len(parts) >= 2:
            invoice = next((i for i in INVOICES if i["id"] == parts[1]), None)
            if invoice is None:
                return self.send_json(404, error("not_found", f"no invoice {parts[1]}"))
            action = parts[2] if len(parts) == 3 else None
            if self.command == "GET" and action is None:
                return self.send_json(200, invoice)
            if self.command == "GET" and action == "pdf":
                self.send_response(200)
                self.send_header("Content-Type", "application/pdf")
                self.send_header("Content-Length", str(len(PDF)))
                self.end_headers()
                return self.wfile.write(PDF)
            if self.command == "POST" and action == "finalize":
                return self.send_json(200, {**invoice, "status": "open"})
            if self.command == "POST" and action == "void":
                if invoice["status"] != "open":
                    return self.send_json(409, error("invalid_state", f"invoice {invoice['id']} is {invoice['status']}, not open"))
                return self.send_json(200, {**invoice, "status": "void"})

        if route == ("GET", "/payments"):
            page = int(query.get("page", ["1"])[0])
            return self.send_json(200, {"items": PAYMENTS[(page - 1) * 2:page * 2], "total_pages": 3})
        if parts[0] == "payments" and len(parts) == 2:
            payment = next((p for p in PAYMENTS if p["id"] == parts[1]), None)
            if payment is None:
                return self.send_json(404, error("not_found", f"no payment {parts[1]}"))
            return self.send_json(200, payment)

        if route == ("POST", "/refunds"):
            data = json.loads(body or b"{}")
            if data.get("amount", 0) > 500:
                return self.send_json(400, error("amount_too_large", "refund exceeds the 500 left on the payment", "amount"))
            return self.send_json(201, {"id": "re_1", "amount": data.get("amount", 500), **data})

        if route == ("POST", "/files"):
            marker = b'filename="'
            start = body.find(marker)
            name = body[start + len(marker):body.find(b'"', start + len(marker))].decode() if start >= 0 else "upload"
            return self.send_json(201, {"id": "file_1", "filename": name, "size_bytes": len(body)})
        if route == ("POST", "/files/raw"):
            return self.send_json(201, {"id": "file_2", "filename": query.get("filename", ["upload"])[0],
                                        "size_bytes": len(body)})

        if route == ("GET", "/events/stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(b": connected\n\n")
            for event in EVENTS:
                self.wfile.write(f"event: message\ndata: {json.dumps(event)}\n\n".encode())
                self.wfile.flush()
            self.close_connection = True
            return

        self.send_json(404, error("not_found", f"no route {self.command} {path}"))

    do_GET = do_POST = do_PATCH = do_DELETE = do_PUT = handle_any


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
