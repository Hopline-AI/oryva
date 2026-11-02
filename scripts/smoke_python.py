"""Checks for scripts/smoke-python.sh: the generated Ledger SDK and MCP server against the shared
stub (scripts/stub_ledger.py), one section per row of the behaviour contract in CLAUDE.md.

Most checks read back what the stub *recorded* (headers, query, raw bodies), not just what the
SDK returned. Usage: python -I smoke_python.py <venv bin dir>
"""

import asyncio
import base64
import hashlib
import hmac
import json
import os
import subprocess
import sys
import tempfile
import time
import warnings
from pathlib import Path

import httpx
from mcp.client import Client
from mcp.client.stdio import StdioServerParameters

import ledger
import ledger._auth as auth_module
from ledger import (
    NOT_GIVEN,
    AsyncLedger,
    BadRequestError,
    ConflictError,
    InternalServerError,
    Ledger,
    LedgerError,
    NotFoundError,
    SyncPage,
    WebhookVerificationError,
    __version__,
)
from ledger.types import (
    Customer,
    InvoicePaidEvent,
    LineItem,
    PaymentFailedEvent,
    UnknownEvent,
    UnknownWebhookEvent,
)

BIN_DIR = sys.argv[1]
STUB = Path(__file__).resolve().parent / "stub_ledger.py"

stub = subprocess.Popen([sys.executable, "-I", str(STUB)], stdout=subprocess.PIPE, text=True)
assert stub.stdout is not None
URL = f"http://127.0.0.1:{int(stub.stdout.readline())}"
OAUTH_URL = f"{URL}/oauth/token"
admin = httpx.Client(base_url=URL)

for name in ("LEDGER_API_KEY", "LEDGER_CLIENT_ID", "LEDGER_CLIENT_SECRET", "LEDGER_BASE_URL",
             "LEDGER_OAUTH_TOKEN_URL", "LEDGER_WEBHOOK_SECRET", "LEDGER_LOG"):  # fmt: skip
    os.environ.pop(name, None)


def check(name, ok):
    print(f"  {'ok  ' if ok else 'FAIL'} {name}")
    if not ok:
        raise SystemExit(f"smoke check failed: {name}")


def reset():
    admin.post("/_stub/reset")


def seen(path=None, method=None):
    rows = admin.get("/_stub/requests").json()
    return [r for r in rows if (path is None or r["path"] == path) and (method is None or r["method"] == method)]


def body_of(row):
    return json.loads(row["body"])


def raises(exc_type, fn):
    try:
        fn()
    except exc_type as exc:
        return exc
    return None


def sign(secret, msg_id, timestamp, body):
    """Standard Webhooks signature, computed here and not with the SDK."""
    key = base64.b64decode(secret.removeprefix("whsec_"))
    mac = hmac.new(key, f"{msg_id}.{timestamp}.".encode() + body, hashlib.sha256).digest()
    return base64.b64encode(mac).decode()


SECRET = "whsec_" + base64.b64encode(b"0123456789abcdef0123456789abcdef").decode()
INVOICE = {"id": "inv_9", "customer_id": "cus_1", "status": "paid", "currency": "usd",
           "line_items": [], "total": 10}  # fmt: skip


def delivery(payload, *, timestamp=None, secret=SECRET, tamper=False):
    body = json.dumps(payload).encode()
    ts = str(int(time.time()) if timestamp is None else timestamp)
    headers = {"webhook-id": "msg_1", "webhook-timestamp": ts,
               "webhook-signature": "v1," + sign(secret, "msg_1", ts, body)}  # fmt: skip
    return (body + b" " if tamper else body), headers


def client(**kw):
    return Ledger(api_key="test-key", base_url=URL, **kw)


# ---------------------------------------------------------------- sync


def sync_checks():
    print("sdk (sync)")
    c = client()

    # Telemetry and auth headers
    reset()
    got = c.customers.get(customer_id="cus_1")
    row = seen("/customers/cus_1")[0]
    check("get parses into the model", isinstance(got, Customer) and got.name == "Customer 1")
    check("User-Agent is <name>-<language>/<version>", row["headers"]["user-agent"] == f"ledger-python/{__version__}" == "ledger-python/2026.10.01")
    check("bearer auth header", row["headers"]["authorization"] == "Bearer test-key")
    check("GET sends no idempotency key", "idempotency-key" not in row["headers"])

    # Environments
    check("production is the default", Ledger(api_key="k").base_url == "https://api.ledger.example")
    check("environment picks a server", Ledger(api_key="k", environment="sandbox").base_url == "https://sandbox.ledger.example")
    check("base_url wins over environment", Ledger(api_key="k", environment="sandbox", base_url=URL).base_url == URL)
    os.environ["LEDGER_BASE_URL"] = URL
    check("LEDGER_BASE_URL overrides the default", Ledger(api_key="k").base_url == URL)
    check("environment beats the env var", Ledger(api_key="k", environment="sandbox").base_url == "https://sandbox.ledger.example")
    del os.environ["LEDGER_BASE_URL"]
    exc = raises(LedgerError, lambda: Ledger(api_key="k", environment="staging"))  # type: ignore[arg-type]
    check("unknown environment lists the choices", exc and "production" in str(exc) and "sandbox" in str(exc))

    # Credentials from the environment, and the error when there are none
    exc = raises(LedgerError, lambda: Ledger(base_url=URL))
    check("missing credentials name every option", exc and all(s in str(exc) for s in ("LEDGER_API_KEY", "LEDGER_CLIENT_ID", "LEDGER_CLIENT_SECRET", "api_key=")))
    os.environ["LEDGER_API_KEY"] = "test-key"
    reset()
    with Ledger(base_url=URL) as env_client:
        env_client.customers.get(customer_id="cus_1")
    check("API key falls back to LEDGER_API_KEY", seen("/customers/cus_1")[0]["headers"]["authorization"] == "Bearer test-key")
    del os.environ["LEDGER_API_KEY"]

    # OAuth2 client credentials: one token fetch, cached, refreshed 60 s before expiry
    reset()
    oauth = Ledger(client_id="test-client", client_secret="test-secret", token_url=OAUTH_URL, base_url=URL)
    oauth.customers.get(customer_id="cus_1")
    oauth.customers.get(customer_id="cus_2")
    tokens = seen("/oauth/token")
    api_calls = seen("/customers/cus_1") + seen("/customers/cus_2")
    form = tokens[0]["body"]
    check("OAuth2 token fetched once and reused", len(tokens) == 1 and {r["headers"]["authorization"] for r in api_calls} == {"Bearer tok_1"})
    check("token request is a client_credentials form", "grant_type=client_credentials" in form and "client_id=test-client" in form and "scope=ledger.read+ledger.write" in form)
    real_now = auth_module._now
    offset = [0.0]
    auth_module._now = lambda: real_now() + offset[0]
    try:
        offset[0] = 3500  # a 3600 s token still has 100 s left
        oauth.customers.get(customer_id="cus_1")
        check("token is kept while more than 60 s remain", len(seen("/oauth/token")) == 1)
        offset[0] = 3545  # 55 s left
        oauth.customers.get(customer_id="cus_1")
        check("token is refreshed 60 s before expiry", len(seen("/oauth/token")) == 2 and seen("/customers/cus_1")[-1]["headers"]["authorization"] == "Bearer tok_2")
    finally:
        auth_module._now = real_now
    os.environ.update(LEDGER_CLIENT_ID="test-client", LEDGER_CLIENT_SECRET="test-secret", LEDGER_OAUTH_TOKEN_URL=OAUTH_URL)
    check("OAuth2 settings fall back to the environment", Ledger(base_url=URL).customers.get(customer_id="cus_1").id == "cus_1")
    for name in ("LEDGER_CLIENT_ID", "LEDGER_CLIENT_SECRET", "LEDGER_OAUTH_TOKEN_URL"):
        del os.environ[name]
    bad = Ledger(client_id="test-client", client_secret="wrong", token_url=OAUTH_URL, base_url=URL)
    check("bad client secret raises AuthenticationError", type(raises(LedgerError, lambda: bad.customers.list())).__name__ == "AuthenticationError")

    # The other auth schemes, as the generated code builds them
    check("API key in a header, query or cookie", all((
        auth_module.api_key_auth("header", "X-Api-Key", "k").credentials(httpx.Client()).headers == {"X-Api-Key": "k"},
        auth_module.api_key_auth("query", "key", "k").credentials(httpx.Client()).params == {"key": "k"},
        auth_module.api_key_auth("cookie", "sid", "k").credentials(httpx.Client()).headers == {"Cookie": "sid=k"},
    )))  # fmt: skip
    basic = auth_module.basic_auth("user", "pass").credentials(httpx.Client()).headers["Authorization"]
    check("basic auth header", basic == "Basic " + base64.b64encode(b"user:pass").decode())

    # Pagination
    reset()
    page = c.customers.list(limit=2)
    check("cursor page exposes items and the next call", isinstance(page, SyncPage) and [x.id for x in page.items] == ["cus_1", "cus_2"] and page.has_next_page() and page.next_page_info() == {"starting_after": "cus_2"})
    check("next_page() fetches the following page", [x.id for x in page.next_page().items] == ["cus_3", "cus_4"])
    reset()
    check("iterating walks every item across pages", [x.id for x in c.customers.list(limit=2)] == [f"cus_{i}" for i in range(1, 6)])
    check("cursor is sent back as starting_after", [r["query"].get("starting_after") for r in seen("/customers", "GET")] == [None, "cus_2", "cus_4"])
    last_page = c.customers.list(limit=2).next_page().next_page()
    check("last page has no next page", not last_page.has_next_page() and raises(LedgerError, last_page.next_page) is not None)
    check("a page with no cursor stops", len(list(c.customers.list(limit=100))) == 5)
    reset()
    invoices = list(c.invoices.list(limit=2))
    check("offset pagination walks every invoice", [i.id for i in invoices] == [f"inv_{n}" for n in range(1, 6)])
    check("offset advances by items received", [r["query"].get("offset") for r in seen("/invoices", "GET")] == [None, "2", "4", "5"])
    reset()
    check("page pagination walks every payment", [p.id for p in c.payments.list()] == [f"pay_{n}" for n in range(1, 7)])
    check("page number counts up", [r["query"].get("page") for r in seen("/payments", "GET")] == [None, "2", "3", "4"])
    reset()
    list(c.payments.list(invoice_id="inv_3"))
    check("filters are kept across pages", {r["query"].get("invoice_id") for r in seen("/payments", "GET")} == {"inv_3"})
    check("total customers by paging", sum(1 for _ in c.customers.list(limit=2)) == 5)

    # Forward compatibility
    odd = c.invoices.get(invoice_id="inv_5")
    check("unknown enum values are accepted and kept", odd.status == "uncollectible" and odd.reminder_level == 3)
    check("known enum values still work", c.invoices.get(invoice_id="inv_4").reminder_level == 2)
    check("unknown object fields are kept", Customer.model_validate({"id": "x", "name": "n", "created_at": "2026-10-01T00:00:00Z", "tier": "gold"}).model_extra == {"tier": "gold"})

    # Omitted vs null, and per-request options
    reset()
    c.customers.update(customer_id="cus_1", email=None)
    c.customers.update(customer_id="cus_1", name="New")
    c.customers.update(customer_id="cus_1", name="N", extra_body={"nickname": "n"}, extra_query={"dry": "1"}, extra_headers={"x-custom": "yes"})
    rows = seen("/customers/cus_1", "PATCH")
    check("null is sent as null", rows[0]["body"] == '{"email": null}' or body_of(rows[0]) == {"email": None})
    check("omitted arguments are not sent", body_of(rows[1]) == {"name": "New"})
    check("extra_body, extra_query and extra_headers", body_of(rows[2]) == {"name": "N", "nickname": "n"} and rows[2]["query"] == {"dry": "1"} and rows[2]["headers"]["x-custom"] == "yes")
    reset()
    c.customers.create(name="A", email=None)
    c.invoices.create(customer_id="cus_1", line_items=[LineItem(description="x", amount=5)])
    c.customers.list(email=None)
    check("null stays in the body, omitted fields stay out", body_of(seen("/customers", "POST")[0]) == {"name": "A", "email": None})
    check("model fields left unset are not sent", body_of(seen("/invoices", "POST")[0]) == {"customer_id": "cus_1", "line_items": [{"description": "x", "amount": 5}]})
    check("None is left out of the query", seen("/customers", "GET")[0]["query"] == {})
    check("NOT_GIVEN is the default", NOT_GIVEN is not None and not NOT_GIVEN)
    check("unknown options are rejected", raises(TypeError, lambda: c.customers.get(customer_id="cus_1", bogus=1)) is not None)

    # Retries and idempotency keys
    reset()
    fail = {"x-stub-fail": "503:2", "x-stub-request": "create-1"}
    made = c.customers.create(name="Retry", extra_headers=fail)
    posts = seen("/customers", "POST")
    keys = [r["headers"].get("idempotency-key") for r in posts]
    check("503 twice then success", len(posts) == 3 and made.name == "Retry")
    check("idempotency key is reused across retries", keys[0] and len(set(keys)) == 1)
    c.customers.create(name="Next")
    check("each call gets a fresh key", seen("/customers", "POST")[-1]["headers"]["idempotency-key"] != keys[0])
    c.customers.create(name="Mine", idempotency_key="my-key-1")
    check("callers can pass their own key", seen("/customers", "POST")[-1]["headers"]["idempotency-key"] == "my-key-1")
    c.customers.create(name="Header", extra_headers={"Idempotency-Key": "from-header"})
    check("a key in extra_headers is respected", seen("/customers", "POST")[-1]["headers"]["idempotency-key"] == "from-header")
    reset()
    exc = raises(InternalServerError, lambda: c.customers.create(name="X", max_retries=1, extra_headers={"x-stub-fail": "503:5", "x-stub-request": "r2"}))
    check("max_retries per request", exc is not None and len(seen("/customers", "POST")) == 2 and exc.status_code == 503)
    check("max_retries=0 sends once", raises(InternalServerError, lambda: c.customers.create(name="X", max_retries=0, extra_headers={"x-stub-fail": "500:1", "x-stub-request": "r3"})) and len(seen("/customers", "POST")) == 3)
    reset()
    limited = Ledger(api_key="test-key", base_url=URL, max_retries=1)
    exc = raises(LedgerError, lambda: limited.customers.list(extra_headers={"x-stub-fail": "429:5", "x-stub-request": "r4"}))
    check("max_retries on the client", type(exc).__name__ == "RateLimitError" and len(seen("/customers", "GET")) == 2)
    reset()
    started = time.monotonic()
    c.customers.list(extra_headers={"x-stub-fail": "503:1", "x-stub-request": "r5", "x-stub-retry-after-date": "2"})
    waited = time.monotonic() - started
    check("Retry-After as an HTTP date is honoured", len(seen("/customers", "GET")) == 2 and 0.9 < waited < 6)
    from ledger._base import retry_delay

    check("Retry-After is capped at 60 s", retry_delay(0, 3600) == 60.0 and retry_delay(0, 0.0) == 0.0)
    reset()
    c.customers.list(extra_headers={"x-stub-fail": "408:1", "x-stub-request": "r6"})
    check("408 is retried", len(seen("/customers", "GET")) == 2)
    exc = raises(BadRequestError, lambda: c.refunds.create(payment_id="pay_1", amount=600))
    check("400 is not retried and carries the server message", exc and len(seen("/refunds")) == 1 and exc.status_code == 400 and "refund exceeds" in str(exc) and exc.body["error"]["param"] == "amount" and exc.request_id.startswith("req_") and "Hint:" in str(exc))
    check("404 raises NotFoundError", raises(NotFoundError, lambda: c.customers.get(customer_id="nope")) is not None)
    exc = raises(ConflictError, lambda: c.invoices.void(invoice_id="inv_1", max_retries=0))
    check("409 raises ConflictError", exc is not None and exc.status_code == 409)
    exc = raises(LedgerError, lambda: Ledger(api_key="test-key", base_url="http://127.0.0.1:1", max_retries=0).customers.list())
    check("connection errors raise APIConnectionError", type(exc).__name__ == "APIConnectionError")
    check("401 names the credentials", "LEDGER_API_KEY" in str(raises(LedgerError, lambda: Ledger(api_key="wrong", base_url=URL).customers.list())))

    # Raw responses
    raw = c.with_raw_response.customers.get(customer_id="cus_1")
    check("raw response has status, headers and parsed body", raw.status_code == 200 and raw.headers["x-request-id"].startswith("req_") and raw.parsed.id == "cus_1")
    check("raw response of a page", c.with_raw_response.customers.list(limit=2).parsed.items[0].id == "cus_1")
    check("raw response of an empty body", c.with_raw_response.customers.delete(customer_id="cus_1").status_code == 204)
    check("delete returns None", c.customers.delete(customer_id="cus_1") is None)

    # Files
    reset()
    c.files.upload(file=b"RECEIPT-BYTES", purpose="receipt")
    row = seen("/files")[0]
    check("multipart upload from bytes", row["headers"]["content-type"].startswith("multipart/form-data") and "RECEIPT-BYTES" in row["body"] and 'name="file"' in row["body"] and 'name="purpose"' in row["body"] and "receipt" in row["body"])
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "receipt.txt"
        path.write_bytes(b"FROM-PATH")
        check("upload from a path", c.files.upload(file=path).filename == "receipt.txt" and "FROM-PATH" in seen("/files")[-1]["body"])
        with path.open("rb") as handle:
            c.files.upload(file=handle)
        check("upload from a file object", "FROM-PATH" in seen("/files")[-1]["body"] and 'filename="receipt.txt"' in seen("/files")[-1]["body"])
        check("optional multipart field left out", 'name="purpose"' not in seen("/files")[-1]["body"])
        stored = c.files.upload_raw(filename="r.bin", file=b"abc")
        row = seen("/files/raw")[0]
        check("raw binary upload", row["body"] == "abc" and row["headers"]["content-type"] == "application/octet-stream" and row["query"] == {"filename": "r.bin"} and stored.size_bytes == 3)
        path.write_bytes(b"\x00\x01\x02")
        c.files.upload_raw(filename="p.bin", file=path)
        check("raw upload from a path", seen("/files/raw")[-1]["body"] == "\x00\x01\x02")
        pdf = c.invoices.download_pdf(invoice_id="inv_1")
        check("download returns bytes", pdf.startswith(b"%PDF") and seen("/invoices/inv_1/pdf")[0]["headers"]["accept"] == "application/pdf")
        out = Path(tmp) / "invoice.pdf"
        check("download streams to a file", c.invoices.download_pdf_to_file(out, invoice_id="inv_1") is None and out.read_bytes() == pdf and not Path(str(out) + ".part").exists())
        missing = Path(tmp) / "none.pdf"
        raises(NotFoundError, lambda: c.invoices.download_pdf_to_file(missing, invoice_id="nope"))
        check("a failed download leaves no file", not missing.exists() and not Path(str(missing) + ".part").exists())

    # Streaming
    with c.events.stream() as stream:
        events = list(stream)
    check("stream yields typed events", [type(e) for e in events] == [InvoicePaidEvent, PaymentFailedEvent, UnknownEvent])
    check("known events are fully typed", events[0].invoice.id == "inv_3" and events[1].payment.failure_reason == "card_declined")
    check("an unknown event type is kept", events[2].type == "customer.created" and events[2].model_extra["customer"]["id"] == "cus_1")
    check("stream asks for text/event-stream", seen("/events/stream")[0]["headers"]["accept"] == "text/event-stream")
    with c.events.stream() as stream:
        first = next(iter(stream))
    check("a stream closes early", isinstance(first, InvoicePaidEvent) and stream.response.is_closed)
    check("raw response of a stream", next(iter(c.with_raw_response.events.stream().parsed)).type == "invoice.paid")
    sse = (b": comment\n\nevent: ping\ndata: " + json.dumps({"type": "customer.created"}).encode() + b"\n\nid: 5\nretry: 10\n\ndata: [DONE]\n\n"
           b"data: " + json.dumps({"type": "invoice.paid", "invoice": INVOICE}).encode() + b"\r\n\r\n")  # fmt: skip
    from ledger._streaming import Stream
    from ledger.types import Event

    odd_stream = list(Stream(cast_to=Event, response=httpx.Response(200, content=sse)))
    check("comments, unknown SSE fields and non-JSON data are skipped", [type(e) for e in odd_stream] == [UnknownEvent, InvoicePaidEvent])

    # Webhooks
    wh = Ledger(api_key="k", webhook_secret=SECRET).webhooks
    body, headers = delivery({"type": "invoice.paid", "invoice": INVOICE})
    event = wh.unwrap(body, headers)
    check("unwrap returns the typed event", isinstance(event, InvoicePaidEvent) and event.invoice.id == "inv_9")
    check("unwrap takes str bodies and any header case", wh.unwrap(body.decode(), {k.title(): v for k, v in headers.items()}).invoice.id == "inv_9")
    failed = wh.unwrap(*delivery({"type": "payment.failed", "payment": {"id": "p", "invoice_id": "i", "amount": 1, "status": "failed"}}))
    check("other event types map to their model", isinstance(failed, PaymentFailedEvent))
    check("unknown webhook events are kept", isinstance(wh.unwrap(*delivery({"type": "refund.created", "x": 1})), UnknownWebhookEvent))
    good = headers["webhook-signature"]
    check("any matching signature in the list is enough", wh.unwrap(body, {**headers, "webhook-signature": f"v1,AAAA {good}"}).invoice.id == "inv_9")
    check("bad signature raises", raises(WebhookVerificationError, lambda: wh.unwrap(body, {**headers, "webhook-signature": "v1,AAAA"})) is not None)
    check("tampered body raises", raises(WebhookVerificationError, lambda: wh.unwrap(*delivery({"type": "invoice.paid", "invoice": INVOICE}, tamper=True))) is not None)
    other = "whsec_" + base64.b64encode(b"another-secret-another-secret-00").decode()
    check("wrong secret raises", raises(WebhookVerificationError, lambda: wh.unwrap(*delivery({"type": "invoice.paid"}, secret=other))) is not None)
    check("timestamp 6 minutes old raises", raises(WebhookVerificationError, lambda: wh.unwrap(*delivery({"type": "invoice.paid"}, timestamp=int(time.time()) - 360))) is not None)
    check("timestamp 6 minutes ahead raises", raises(WebhookVerificationError, lambda: wh.unwrap(*delivery({"type": "invoice.paid"}, timestamp=int(time.time()) + 360))) is not None)
    check("timestamp 4 minutes old is fine", wh.unwrap(*delivery({"type": "invoice.paid", "invoice": INVOICE}, timestamp=int(time.time()) - 240)).invoice.id == "inv_9")
    check("missing headers raise", raises(WebhookVerificationError, lambda: wh.unwrap(body, {})) is not None)
    check("no secret raises with a fix", "LEDGER_WEBHOOK_SECRET" in str(raises(WebhookVerificationError, lambda: Ledger(api_key="k").webhooks.unwrap(body, headers))))
    os.environ["LEDGER_WEBHOOK_SECRET"] = SECRET
    check("secret falls back to LEDGER_WEBHOOK_SECRET", Ledger(api_key="k").webhooks.unwrap(body, headers).invoice.id == "inv_9")
    del os.environ["LEDGER_WEBHOOK_SECRET"]

    # Deprecation
    ledger._utils._warned.clear()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        found = c.customers.search(q="customer")
        c.customers.search(q="customer")
    deprecations = [w for w in caught if issubclass(w.category, DeprecationWarning)]
    check("deprecated methods still work and warn once", len(found) == 5 and len(deprecations) == 1 and "customers.list" in str(deprecations[0].message) and deprecations[0].filename == __file__)

    # Debug logging with credentials redacted
    env = {"PATH": os.environ["PATH"], "URL": URL}
    code = (
        "import os\nfrom ledger import Ledger\n"
        "o = Ledger(client_id='test-client', client_secret='test-secret', token_url=os.environ['URL'] + '/oauth/token', base_url=os.environ['URL'])\n"
        "o.customers.get(customer_id='cus_1')\n"
        "Ledger(api_key='test-key', base_url=os.environ['URL']).customers.list(limit=1)\n"
    )
    quiet = subprocess.run([sys.executable, "-I", "-c", code], env=env, capture_output=True, text=True)
    loud = subprocess.run([sys.executable, "-I", "-c", code], env={**env, "LEDGER_LOG": "debug"}, capture_output=True, text=True)
    check("no logging by default", quiet.returncode == 0 and quiet.stderr == "")
    check("LEDGER_LOG=debug logs requests", loud.returncode == 0 and "Request: GET" in loud.stderr and "Response: 200" in loud.stderr)
    check("credentials are redacted", "[redacted]" in loud.stderr and not any(s in loud.stderr for s in ("test-key", "test-secret", "tok_1")))


# ---------------------------------------------------------------- async


async def async_checks():
    print("sdk (async)")
    reset()
    async with AsyncLedger(api_key="test-key", base_url=URL) as c:
        got = await c.customers.get(customer_id="cus_1")
        row = seen("/customers/cus_1")[0]
        check("async get, User-Agent and bearer header", got.id == "cus_1" and row["headers"]["user-agent"] == "ledger-python/2026.10.01" and row["headers"]["authorization"] == "Bearer test-key")
        check("async environments", AsyncLedger(api_key="k", environment="sandbox").base_url == "https://sandbox.ledger.example")

        reset()
        page = await c.customers.list(limit=2)
        check("async page exposes items and next page", [x.id for x in page.items] == ["cus_1", "cus_2"] and page.next_page_info() == {"starting_after": "cus_2"} and [x.id for x in (await page.next_page()).items] == ["cus_3", "cus_4"])
        check("async iteration walks every customer", [x.id async for x in await c.customers.list(limit=2)] == [f"cus_{i}" for i in range(1, 6)])
        check("async offset pagination", [i.id async for i in await c.invoices.list(limit=2)] == [f"inv_{n}" for n in range(1, 6)])
        check("async page pagination", [p.id async for p in await c.payments.list()] == [f"pay_{n}" for n in range(1, 7)])

        odd = await c.invoices.get(invoice_id="inv_5")
        check("async forward-compatible enums", odd.status == "uncollectible" and odd.reminder_level == 3)

        reset()
        await c.customers.update(customer_id="cus_1", email=None)
        await c.customers.update(customer_id="cus_1", name="N", extra_body={"z": 1}, extra_query={"q": "1"}, extra_headers={"x-custom": "a"})
        rows = seen("/customers/cus_1", "PATCH")
        check("async null vs omitted and extra options", body_of(rows[0]) == {"email": None} and body_of(rows[1]) == {"name": "N", "z": 1} and rows[1]["query"] == {"q": "1"} and rows[1]["headers"]["x-custom"] == "a")

        reset()
        await c.customers.create(name="Retry", extra_headers={"x-stub-fail": "503:2", "x-stub-request": "a1"})
        keys = {r["headers"]["idempotency-key"] for r in seen("/customers", "POST")}
        check("async retries reuse one idempotency key", len(seen("/customers", "POST")) == 3 and len(keys) == 1)
        await c.customers.create(name="Mine", idempotency_key="async-key")
        check("async caller-supplied key", seen("/customers", "POST")[-1]["headers"]["idempotency-key"] == "async-key")
        started = time.monotonic()
        await c.customers.list(extra_headers={"x-stub-fail": "503:1", "x-stub-request": "a2", "x-stub-retry-after-date": "2"})
        check("async Retry-After as an HTTP date", 0.9 < time.monotonic() - started < 6)
        try:
            await c.refunds.create(payment_id="pay_1", amount=600)
            check("async 400 raises", False)
        except BadRequestError as exc:
            check("async errors carry the server message", "refund exceeds" in str(exc))

        raw = await c.with_raw_response.customers.get(customer_id="cus_1")
        check("async raw response", raw.status_code == 200 and "x-request-id" in raw.headers and raw.parsed.id == "cus_1")

        await c.files.upload(file=b"ASYNC-BYTES", purpose="contract")
        await c.files.upload_raw(filename="a.bin", file=b"xyz")
        check("async uploads", "ASYNC-BYTES" in seen("/files")[-1]["body"] and seen("/files/raw")[-1]["body"] == "xyz")
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "a.pdf"
            await c.invoices.download_pdf_to_file(out, invoice_id="inv_1")
            check("async download as bytes and to a file", (await c.invoices.download_pdf(invoice_id="inv_1")) == out.read_bytes() and out.read_bytes().startswith(b"%PDF"))

        async with await c.events.stream() as stream:
            events = [e async for e in stream]
        check("async stream yields typed events", [type(e) for e in events] == [InvoicePaidEvent, PaymentFailedEvent, UnknownEvent])
        async with await c.events.stream() as stream:
            async for event in stream:
                break
        check("async stream closes early", isinstance(event, InvoicePaidEvent) and stream.response.is_closed)

        ledger._utils._warned.clear()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            await c.customers.search(q="customer")
            await c.customers.search(q="customer")
        check("async deprecated method warns once", len([w for w in caught if issubclass(w.category, DeprecationWarning)]) == 1)

        body, headers = delivery({"type": "invoice.paid", "invoice": INVOICE})
        check("async client verifies webhooks", AsyncLedger(api_key="k", webhook_secret=SECRET).webhooks.unwrap(body, headers).invoice.id == "inv_9")

    reset()
    oauth = AsyncLedger(client_id="test-client", client_secret="test-secret", token_url=OAUTH_URL, base_url=URL)
    await oauth.customers.get(customer_id="cus_1")
    await oauth.customers.get(customer_id="cus_2")
    api_calls = seen("/customers/cus_1") + seen("/customers/cus_2")
    check("async OAuth2 fetches the token once", len(seen("/oauth/token")) == 1 and {r["headers"]["authorization"] for r in api_calls} == {"Bearer tok_1"})
    real_now = auth_module._now
    auth_module._now = lambda: real_now() + 3545
    try:
        await oauth.customers.get(customer_id="cus_1")
        check("async OAuth2 refreshes before expiry", len(seen("/oauth/token")) == 2)
    finally:
        auth_module._now = real_now
    await oauth.aclose()


# ---------------------------------------------------------------- mcp


def text_of(result):
    return result.content[0].text


async def mcp_checks():
    print("mcp")
    out_dir = tempfile.mkdtemp(prefix="ledger-mcp-out-")
    params = StdioServerParameters(
        command=os.path.join(BIN_DIR, "ledger-mcp"),
        env={"LEDGER_API_KEY": "test-key", "LEDGER_BASE_URL": URL, "LEDGER_MCP_OUTPUT_DIR": out_dir},
    )
    reset()
    async with Client(params) as client:
        tools = {t.name: t for t in (await client.list_tools()).tools}
        expected = {
            "customers_list", "customers_create", "customers_get", "customers_update", "customers_delete",
            "invoices_list", "invoices_create", "invoices_get", "invoices_finalize", "invoices_void", "invoices_download_pdf",
            "payments_list", "payments_get", "refunds_create", "files_upload", "files_upload_raw", "events_stream",
        }  # fmt: skip
        check("tool names", set(tools) == expected)
        flags = lambda n: (tools[n].annotations.read_only_hint, tools[n].annotations.destructive_hint, tools[n].annotations.idempotent_hint)  # noqa: E731
        check("customers_delete is destructive", flags("customers_delete") == (False, True, True))
        check("invoices_void is destructive and idempotent", flags("invoices_void") == (False, True, True))
        check("refunds_create is destructive, not idempotent", flags("refunds_create") == (False, True, False))
        check("customers_list is read only", flags("customers_list") == (True, False, True))
        check("events_stream is read only", tools["events_stream"].annotations.read_only_hint is True)
        check("invoices_finalize is a plain write", flags("invoices_finalize") == (False, False, False))
        check("hint is in the description", "Fetch the customer first" in tools["customers_delete"].description and "Check the payment's amount first" in tools["refunds_create"].description)
        schema = json.dumps(tools["invoices_create"].input_schema)
        check("enums and defaults are in the input schema", all(v in schema for v in ("usd", "gbp")) and "max_events" in json.dumps(tools["events_stream"].input_schema))
        check("paginated tool says how to get the next page", "next_page" in tools["customers_list"].description)

        result = await client.call_tool("customers_list", {"limit": 2})
        first = json.loads(text_of(result))
        check("paginated tool returns one page and the next argument", not result.is_error and [c["id"] for c in first["data"]] == ["cus_1", "cus_2"] and first["next_page"] == {"starting_after": "cus_2"} and len(seen("/customers", "GET")) == 1)
        second = json.loads(text_of(await client.call_tool("customers_list", {"limit": 2, **first["next_page"]})))
        third = json.loads(text_of(await client.call_tool("customers_list", {"limit": 2, **second["next_page"]})))
        check("following next_page reaches the end", [c["id"] for c in second["data"]] == ["cus_3", "cus_4"] and [c["id"] for c in third["data"]] == ["cus_5"] and third["next_page"] is None)
        offset = json.loads(text_of(await client.call_tool("invoices_list", {"limit": 3})))
        check("offset tool returns the next offset", offset["next_page"] == {"offset": 3} and offset["total"] == 5)
        paged = json.loads(text_of(await client.call_tool("payments_list", {"page": 2})))
        check("page tool returns the next page number", paged["next_page"] == {"page": 3} and paged["items"][0]["id"] == "pay_3")

        result = await client.call_tool("events_stream", {"max_events": 2})
        got = json.loads(text_of(result))
        check("stream tool stops at max_events", got["count"] == 2 and [e["type"] for e in got["events"]] == ["invoice.paid", "payment.failed"])
        got = json.loads(text_of(await client.call_tool("events_stream", {})))
        check("stream tool reads up to 10 events by default", got["count"] == 3 and got["events"][2]["type"] == "customer.created")

        result = await client.call_tool("invoices_download_pdf", {"invoice_id": "inv_1"})
        saved = json.loads(text_of(result))
        check("binary download is saved to the output dir", saved["path"].startswith(out_dir) and Path(saved["path"]).read_bytes().startswith(b"%PDF") and saved["content_type"] == "application/pdf")

        reset()
        await client.call_tool("customers_create", {"name": "Zed"})
        check("omitted optional arguments are not sent as null", body_of(seen("/customers", "POST")[0]) == {"name": "Zed"})
        result = await client.call_tool("refunds_create", {"payment_id": "pay_1", "amount": 600})
        check("SDK errors come back as tool errors with the SDK message", result.is_error and "refund exceeds" in text_of(result))
        result = await client.call_tool("customers_delete", {"customer_id": "cus_1"})
        check("empty responses succeed", not result.is_error)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "r.txt"
            path.write_text("MCP-FILE")
            await client.call_tool("files_upload", {"file": str(path), "purpose": "receipt"})
            check("file arguments are paths on the server's machine", "MCP-FILE" in seen("/files")[-1]["body"])

    reset()
    params = StdioServerParameters(
        command=os.path.join(BIN_DIR, "ledger-mcp"),
        env={"LEDGER_CLIENT_ID": "test-client", "LEDGER_CLIENT_SECRET": "test-secret", "LEDGER_OAUTH_TOKEN_URL": OAUTH_URL, "LEDGER_BASE_URL": URL},
    )
    async with Client(params) as client:
        result = await client.call_tool("customers_get", {"customer_id": "cus_1"})
        check("MCP credentials come from env vars for OAuth2 too", not result.is_error and len(seen("/oauth/token")) == 1 and seen("/customers/cus_1")[0]["headers"]["authorization"] == "Bearer tok_1")


try:
    sync_checks()
    asyncio.run(async_checks())
    asyncio.run(mcp_checks())
finally:
    stub.kill()
