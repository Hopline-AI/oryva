"""Checks for scripts/smoke-python.sh: the generated SDK and MCP server against a stub API."""

import asyncio
import io
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

from mcp.client import Client
from mcp.client.stdio import StdioServerParameters

from sarvam import (
    BadRequestError,
    Sarvam,
    SarvamError,
    __version__,
)
from sarvam.types import TranslateResponse

BIN_DIR = sys.argv[1]
requests = []  # (method, path, headers, body)
calls = {}  # input text -> number of /translate calls


class Stub(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, status, body=None, headers=None, raw=None):
        payload = raw if raw is not None else (json.dumps(body).encode() if body is not None else b"")
        self.send_response(status)
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        if payload:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def handle_any(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        requests.append((self.command, self.path, dict(self.headers), body))
        if self.headers.get("api-subscription-key") != "test-key":
            return self.reply(401, {"error": {"message": "bad key", "code": "auth"}})
        if self.path == "/translate":
            text = json.loads(body)["input"]
            calls[text] = calls.get(text, 0) + 1
            if text == "bad":
                err = {"message": "input too long", "code": "bad_input", "request_id": "req_1"}
                return self.reply(400, {"error": err})
            if text == "retry-me" and calls[text] == 1:
                return self.reply(429, {"error": {"message": "slow down", "code": "rate"}}, {"Retry-After": "0"})
            return self.reply(200, {"translated_text": "namaste", "source_language_code": "en-IN"})
        if self.path == "/text-lid":
            return self.reply(200, {"language_code": "hi-IN", "script_code": None})
        if self.path == "/speech-to-text":
            return self.reply(200, {"transcript": "hello", "language_code": "en-IN"})
        if self.path == "/pronunciations/p1" and self.command == "DELETE":
            return self.reply(204)
        self.reply(404, {"error": {"message": f"no route {self.path}", "code": "nf"}})

    do_GET = do_POST = do_DELETE = handle_any


server = HTTPServer(("127.0.0.1", 0), Stub)
threading.Thread(target=server.serve_forever, daemon=True).start()
URL = f"http://127.0.0.1:{server.server_port}"

TRANSLATE = dict(source_language_code="en-IN", target_language_code="hi-IN")


def check(name, ok):
    print(f"  {'ok  ' if ok else 'FAIL'} {name}")
    if not ok:
        raise SystemExit(f"smoke check failed: {name}")


def sdk_checks():
    print("sdk")
    client = Sarvam(api_key="test-key", base_url=URL)
    check("version is exported", __version__ == "1.0")

    result = client.text.translate(input="hello", **TRANSLATE)
    check("translate parses into the model", isinstance(result, TranslateResponse) and result.translated_text == "namaste")
    sent = [r for r in requests if r[1] == "/translate"][-1]
    check("api key header and JSON body are sent", json.loads(sent[3]) == {"input": "hello", **TRANSLATE})

    result = client.text.translate(input="retry-me", **TRANSLATE)
    check("429 then 200 is retried", calls["retry-me"] == 2 and result.translated_text == "namaste")

    try:
        client.text.translate(input="bad", **TRANSLATE)
        check("400 raises", False)
    except BadRequestError as exc:
        check(
            "400 raises BadRequestError with the server message",
            exc.status_code == 400
            and exc.request_id == "req_1"
            and "POST /translate returned 400: input too long (request_id=req_1)" in str(exc),
        )
    check("400 is not retried", calls["bad"] == 1)

    exhausted = Sarvam(api_key="wrong", base_url=URL)
    try:
        exhausted.text.translate(input="hello", **TRANSLATE)
        check("401 raises", False)
    except SarvamError as exc:
        check("401 names the env var", "SARVAM_API_KEY" in str(exc))

    check("delete returns None", client.pronunciations.delete(pronunciation_id="p1") is None)

    wav = b"RIFF....WAVEfmt "
    client.speech_to_text.transcribe(file=wav, language_code="hi-IN")
    method, path, headers, body = requests[-1]
    check(
        "transcribe sends multipart with the file",
        headers["Content-Type"].startswith("multipart/form-data")
        and wav in body
        and b'name="file"' in body
        and b'name="language_code"' in body
        and b"hi-IN" in body,
    )
    out = client.speech_to_text.transcribe(file=io.BytesIO(wav))
    check("transcribe accepts a file object", out.transcript == "hello" and wav in requests[-1][3])

    saved = os.environ.pop("SARVAM_API_KEY", None)
    try:
        Sarvam(base_url=URL)
        check("missing key raises", False)
    except SarvamError as exc:
        check("missing key says how to fix it", "SARVAM_API_KEY" in str(exc) and "api_key=" in str(exc))
    os.environ["SARVAM_API_KEY"] = "test-key"
    os.environ["SARVAM_BASE_URL"] = URL
    with Sarvam() as env_client:
        check("key and base URL fall back to the environment", env_client.text.identify_language(input="x").language_code == "hi-IN")
    if saved is not None:
        os.environ["SARVAM_API_KEY"] = saved

    try:
        Sarvam(api_key="test-key", base_url="http://127.0.0.1:1", max_retries=0).text.identify_language(input="x")
        check("connection errors raise", False)
    except SarvamError as exc:
        check("connection error is an APIConnectionError", type(exc).__name__ == "APIConnectionError")


async def mcp_checks():
    print("mcp")
    params = StdioServerParameters(
        command=os.path.join(BIN_DIR, "sarvam-mcp"),
        env={"SARVAM_API_KEY": "test-key", "SARVAM_BASE_URL": URL},
    )
    async with Client(params) as client:
        tools = {t.name: t for t in (await client.list_tools()).tools}
        expected = {
            "text_translate", "text_transliterate", "text_identify_language",
            "text_to_speech_convert", "speech_to_text_transcribe",
            "pronunciations_list", "pronunciations_create", "pronunciations_get", "pronunciations_delete",
        }  # fmt: skip
        check("tool names", set(tools) == expected)
        check("pronunciations_delete is destructive", tools["pronunciations_delete"].annotations.destructive_hint is True)
        check("text_identify_language is read only", tools["text_identify_language"].annotations.read_only_hint is True)
        check("titles are set", tools["text_translate"].annotations.title == "Translate text between Indian languages and English")
        schema = json.dumps(tools["text_translate"].input_schema)
        check("enums are in the input schema", all(v in schema for v in ("hi-IN", "auto", "code-mixed")))
        check(
            "description carries summary and hint",
            "Confirm the ID" in tools["pronunciations_delete"].description
            and tools["pronunciations_delete"].description.startswith("Delete a custom pronunciation."),
        )

        result = await client.call_tool("text_translate", {"input": "hello", **TRANSLATE})
        text = result.content[0].text
        check("text_translate works against the stub", not result.is_error and json.loads(text)["translated_text"] == "namaste")

        result = await client.call_tool("text_translate", {"input": "bad", **TRANSLATE})
        check(
            "SDK errors come back as tool errors with the SDK message",
            result.is_error and "POST /translate returned 400: input too long" in result.content[0].text,
        )

        result = await client.call_tool("pronunciations_delete", {"pronunciation_id": "p1"})
        check("delete tool succeeds", not result.is_error)


sdk_checks()
asyncio.run(mcp_checks())
