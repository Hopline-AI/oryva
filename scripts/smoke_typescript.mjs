// Checks for scripts/smoke-typescript.sh: the generated TypeScript SDK against scripts/stub_ledger.py.
// Run by `node --test` with SDK_DIR pointing at the built package. Expected values come from the
// behaviour contract in CLAUDE.md and from the stub's recorded requests, never from the SDK itself.

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHmac } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { after, before, beforeEach, describe, it } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const sdkDir = process.env.SDK_DIR;
const sdk = await import(pathToFileURL(`${sdkDir}/dist/index.js`).href);
const { Ledger } = sdk;
const version = JSON.parse(readFileSync(`${sdkDir}/package.json`, 'utf8')).version;

for (const name of Object.keys(process.env)) if (name.startsWith('LEDGER_')) delete process.env[name];

let stub;
let base;
before(async () => {
  const stubPath = fileURLToPath(new URL('./stub_ledger.py', import.meta.url));
  stub = spawn('python3', ['-I', stubPath], { stdio: ['ignore', 'pipe', 'inherit'] });
  const port = await new Promise((resolve) => stub.stdout.once('data', (d) => resolve(String(d).trim())));
  base = `http://127.0.0.1:${port}`;
});
after(() => stub?.kill());
beforeEach(async () => {
  await fetch(`${base}/_stub/reset`, { method: 'POST' });
  for (const name of Object.keys(process.env)) if (name.startsWith('LEDGER_')) delete process.env[name];
});

const recorded = async () => (await fetch(`${base}/_stub/requests`)).json();
const client = (options = {}) => new Ledger({ baseURL: base, apiKey: 'test-key', ...options });
const fail = (spec, id) => ({ headers: { 'x-stub-fail': spec, 'x-stub-request': id } });
const json = (body, status = 200, headers = {}) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } });

describe('auth', () => {
  it('sends the bearer key', async () => {
    const customer = await client().customers.get('cus_1');
    assert.equal(customer.id, 'cus_1');
    assert.equal((await recorded())[0].headers.authorization, 'Bearer test-key');
  });

  it('reads the key from LEDGER_API_KEY', async () => {
    process.env.LEDGER_API_KEY = 'test-key';
    await new Ledger({ baseURL: base }).customers.get('cus_1');
    assert.equal((await recorded())[0].headers.authorization, 'Bearer test-key');
  });

  it('fetches an OAuth2 token once and reuses it', async () => {
    const c = new Ledger({ baseURL: base, clientId: 'test-client', clientSecret: 'test-secret', tokenURL: `${base}/oauth/token` });
    await c.customers.get('cus_1');
    await c.customers.get('cus_2');
    const seen = await recorded();
    assert.equal(seen.filter((r) => r.path === '/oauth/token').length, 1);
    const token = seen.filter((r) => r.path.startsWith('/customers')).map((r) => r.headers.authorization);
    assert.deepEqual(token, ['Bearer tok_1', 'Bearer tok_1']);
    const form = new URLSearchParams(seen[0].body);
    assert.equal(form.get('grant_type'), 'client_credentials');
    assert.equal(form.get('scope'), 'ledger.read ledger.write');
  });

  it('takes the OAuth2 credentials and token URL from the environment', async () => {
    process.env.LEDGER_CLIENT_ID = 'test-client';
    process.env.LEDGER_CLIENT_SECRET = 'test-secret';
    process.env.LEDGER_OAUTH_TOKEN_URL = `${base}/oauth/token`;
    await new Ledger({ baseURL: base }).customers.get('cus_1');
    const seen = await recorded();
    assert.equal(seen[0].path, '/oauth/token');
    assert.equal(seen[1].headers.authorization, 'Bearer tok_1');
  });

  it('renews the token a minute before it expires', async () => {
    const shortLived = async (url, init) => {
      const response = await fetch(url, init);
      if (!String(url).endsWith('/oauth/token')) return response;
      return json({ ...(await response.json()), expires_in: 30 });
    };
    const c = new Ledger({ baseURL: base, clientId: 'test-client', clientSecret: 'test-secret', tokenURL: `${base}/oauth/token`, fetch: shortLived });
    await c.customers.get('cus_1');
    await c.customers.get('cus_2');
    assert.equal((await recorded()).filter((r) => r.path === '/oauth/token').length, 2);
  });

  it('prefers the bearer key over OAuth2 when both are set', async () => {
    await client({ clientId: 'test-client', clientSecret: 'test-secret' }).customers.get('cus_1');
    assert.equal((await recorded()).filter((r) => r.path === '/oauth/token').length, 0);
  });

  it('says which variables to set when there are no credentials', async () => {
    await assert.rejects(new Ledger({ baseURL: base }).customers.get('cus_1'), (e) => {
      assert.ok(e instanceof sdk.LedgerError);
      assert.match(e.message, /LEDGER_API_KEY/);
      assert.match(e.message, /LEDGER_CLIENT_ID and LEDGER_CLIENT_SECRET/);
      return true;
    });
    assert.deepEqual(await recorded(), []);
  });

  it('turns a rejected key into AuthenticationError with a request id and a hint', async () => {
    await assert.rejects(client({ apiKey: 'wrong' }).customers.get('cus_1'), (e) => {
      assert.ok(e instanceof sdk.AuthenticationError);
      assert.equal(e.status, 401);
      assert.match(e.requestID, /^req_\d+$/);
      assert.match(e.message, /missing or invalid credentials/);
      assert.match(e.message, /LEDGER_API_KEY/);
      return true;
    });
    assert.equal((await recorded()).length, 1, '401 is not retried');
  });
});

describe('environments', () => {
  it('picks a named server, a base URL, or the env var', () => {
    assert.equal(new Ledger({ apiKey: 'k' }).baseURL, 'https://api.ledger.example');
    assert.equal(new Ledger({ apiKey: 'k', environment: 'sandbox' }).baseURL, 'https://sandbox.ledger.example');
    assert.equal(new Ledger({ apiKey: 'k', baseURL: 'http://x.test/' }).baseURL, 'http://x.test');
    process.env.LEDGER_BASE_URL = 'http://from-env.test';
    assert.equal(new Ledger({ apiKey: 'k' }).baseURL, 'http://from-env.test');
    assert.equal(new Ledger({ apiKey: 'k', environment: 'sandbox' }).baseURL, 'https://sandbox.ledger.example');
    assert.throws(() => new Ledger({ apiKey: 'k', environment: 'staging' }), /production, sandbox/);
  });
});

describe('retries', () => {
  it('retries 503 and succeeds on the third attempt', async () => {
    const customer = await client().customers.get('cus_1', fail('503:2', 'r1'));
    assert.equal(customer.id, 'cus_1');
    assert.equal((await recorded()).length, 3);
  });

  it('gives up after maxRetries, per client and per request', async () => {
    await assert.rejects(client({ maxRetries: 1 }).customers.get('cus_1', fail('503:5', 'r2')), (e) => {
      assert.ok(e instanceof sdk.InternalServerError);
      assert.equal(e.status, 503);
      return true;
    });
    assert.equal((await recorded()).length, 2);
    await assert.rejects(client().customers.get('cus_1', { ...fail('503:5', 'r3'), maxRetries: 0 }));
    assert.equal((await recorded()).length, 3);
  });

  it('retries 409, 429 and 408 but not 400', async () => {
    for (const [status, id] of [[409, 'a'], [429, 'b'], [408, 'c']]) {
      await client().customers.get('cus_1', fail(`${status}:1`, id));
    }
    assert.equal((await recorded()).length, 6);
    await fetch(`${base}/_stub/reset`, { method: 'POST' });
    await assert.rejects(client().customers.get('cus_1', fail('400:1', 'd')), sdk.BadRequestError);
    assert.equal((await recorded()).length, 1);
  });

  // The SDK logs "retrying in <n>ms" at info level; real sleeps are skipped so 60 s caps are testable.
  const withoutSleeping = async (run) => {
    const waits = [];
    const logger = { debug() {}, warn() {}, error() {}, info: (m) => waits.push(Number(/retrying in (\d+)ms/.exec(m)?.[1])) };
    const realSetTimeout = globalThis.setTimeout;
    globalThis.setTimeout = (fn, ms, ...rest) => realSetTimeout(fn, ms > 1000 ? 0 : ms, ...rest);
    try {
      await run(logger);
    } finally {
      globalThis.setTimeout = realSetTimeout;
    }
    return waits;
  };

  it('honours Retry-After seconds, HTTP dates, and caps it at 60 s', async () => {
    const waits = await withoutSleeping(async (logger) => {
      for (const retryAfter of ['2', new Date(Date.now() + 5000).toUTCString(), '3600']) {
        let calls = 0;
        const fake = async () => (calls++ ? json({ id: 'cus_1' }) : json({}, 429, { 'retry-after': retryAfter }));
        await client({ fetch: fake, logger, logLevel: 'info' }).customers.get('cus_1');
      }
    });
    assert.equal(waits[0], 2000);
    assert.ok(waits[1] >= 3000 && waits[1] <= 5000, `HTTP date wait was ${waits[1]}`);
    assert.equal(waits[2], 60000);
  });

  it('backs off exponentially with jitter when there is no Retry-After', async () => {
    const waits = await withoutSleeping((logger) =>
      assert.rejects(client({ fetch: async () => json({}, 500), maxRetries: 3, logger, logLevel: 'info' }).customers.get('cus_1')));
    assert.equal(waits.length, 3);
    [500, 1000, 2000].forEach((max, i) => assert.ok(waits[i] > max * 0.7 && waits[i] <= max, `wait ${i} was ${waits[i]}`));
  });

  it('retries connection errors, then raises APIConnectionError', async () => {
    let calls = 0;
    const down = async () => {
      calls++;
      throw new TypeError('fetch failed');
    };
    await assert.rejects(client({ fetch: down }).customers.get('cus_1'), (e) => {
      assert.ok(e instanceof sdk.APIConnectionError);
      assert.ok(e.cause instanceof TypeError);
      return true;
    });
    assert.equal(calls, 3);
  });

  it('times out with APIConnectionTimeoutError', async () => {
    const hang = (url, init) => new Promise((_, reject) => init.signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))));
    await assert.rejects(client({ fetch: hang }).customers.get('cus_1', { timeout: 30, maxRetries: 0 }), sdk.APIConnectionTimeoutError);
  });

  it('stops on a caller abort without retrying', async () => {
    let calls = 0;
    const hang = (url, init) => {
      calls++;
      return new Promise((_, reject) => init.signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))));
    };
    const controller = new AbortController();
    const pending = client({ fetch: hang }).customers.get('cus_1', { signal: controller.signal });
    setTimeout(() => controller.abort(), 20);
    await assert.rejects(pending, sdk.APIUserAbortError);
    assert.equal(calls, 1);
  });
});

describe('errors', () => {
  it('maps status codes to classes', async () => {
    const cases = { 400: 'BadRequestError', 401: 'AuthenticationError', 403: 'PermissionDeniedError', 404: 'NotFoundError', 409: 'ConflictError', 422: 'UnprocessableEntityError', 429: 'RateLimitError', 500: 'InternalServerError', 418: 'APIError' };
    for (const [status, name] of Object.entries(cases)) {
      const c = client({ maxRetries: 0, fetch: async () => json({ error: { code: 'x', message: `boom ${status}` } }, Number(status), { 'x-request-id': 'req_abc' }) });
      await assert.rejects(c.customers.get('cus_1'), (e) => {
        assert.ok(e instanceof sdk[name], `${status} should be ${name}, got ${e.constructor.name}`);
        assert.ok(e instanceof sdk.APIError && e instanceof sdk.LedgerError && e instanceof Error);
        assert.equal(e.status, Number(status));
        assert.equal(e.requestID, 'req_abc');
        assert.deepEqual(e.error, { error: { code: 'x', message: `boom ${status}` } });
        assert.match(e.message, new RegExp(`boom ${status}`));
        return true;
      });
    }
  });

  it('carries the server message and the offending field', async () => {
    await assert.rejects(client().refunds.create({ payment_id: 'pay_1', amount: 900 }), (e) => {
      assert.ok(e instanceof sdk.BadRequestError);
      assert.match(e.message, /refund exceeds the 500 left/);
      assert.equal(e.error.error.param, 'amount');
      return true;
    });
    await assert.rejects(client().invoices.void('inv_1'), sdk.ConflictError);
  });
});

describe('idempotency', () => {
  it('sends a fresh key per non-GET request and reuses it across retries', async () => {
    const c = client();
    await c.customers.create({ name: 'A' }, fail('503:2', 'i1'));
    const first = (await recorded()).map((r) => r.headers['idempotency-key']);
    assert.equal(first.length, 3);
    assert.ok(first[0]);
    assert.deepEqual(new Set(first).size, 1);
    await c.customers.create({ name: 'B' });
    const second = (await recorded())[3].headers['idempotency-key'];
    assert.ok(second && second !== first[0]);
  });

  it('adds keys to PATCH and DELETE but not to GET', async () => {
    const c = client();
    await c.customers.get('cus_1');
    await c.customers.update('cus_1', { name: 'N' });
    await c.customers.delete('cus_1');
    const keys = (await recorded()).map((r) => r.headers['idempotency-key']);
    assert.equal(keys[0], undefined);
    assert.ok(keys[1] && keys[2] && keys[1] !== keys[2]);
  });

  it('uses the key the caller passes', async () => {
    await client().customers.create({ name: 'A' }, { idempotencyKey: 'my-key-1', ...fail('503:1', 'i2') });
    assert.deepEqual((await recorded()).map((r) => r.headers['idempotency-key']), ['my-key-1', 'my-key-1']);
  });
});

describe('request options', () => {
  it('sends extra headers, removes headers on null, and adds query and body fields', async () => {
    const c = client({ defaultHeaders: { 'x-custom': 'client' } });
    await c.customers.get('cus_1', { headers: { 'x-custom': 'request' } });
    await c.customers.get('cus_1', { headers: { 'x-custom': null } });
    await c.customers.get('cus_1');
    await c.customers.list({ limit: 1 }, { query: { email: 'c1@example.com' } });
    await c.customers.create({ name: 'A' }, { body: { email: 'a@example.com' } });
    const seen = await recorded();
    assert.equal(seen[0].headers['x-custom'], 'request');
    assert.equal(seen[1].headers['x-custom'], undefined);
    assert.equal(seen[2].headers['x-custom'], 'client');
    assert.deepEqual(seen[3].query, { limit: '1', email: 'c1@example.com' });
    assert.deepEqual(JSON.parse(seen[4].body), { name: 'A', email: 'a@example.com' });
  });

  it('sends the User-Agent', async () => {
    await client().customers.get('cus_1');
    assert.equal((await recorded())[0].headers['user-agent'], `ledger-typescript/${version}`);
  });

  it('returns data and the raw Response with withResponse()', async () => {
    const { data, response } = await client().customers.get('cus_1').withResponse();
    assert.equal(data.id, 'cus_1');
    assert.equal(response.status, 200);
    assert.match(response.headers.get('x-request-id'), /^req_/);
    const raw = await client().customers.get('cus_2').asResponse();
    assert.equal((await raw.json()).id, 'cus_2');
  });
});

describe('omitted vs null', () => {
  it('omits undefined and sends null', async () => {
    const c = client();
    await c.customers.update('cus_1', { name: 'N' });
    await c.customers.update('cus_1', { email: null });
    await c.customers.update('cus_1', { name: 'N', email: undefined });
    await c.customers.list({ email: undefined, starting_after: undefined });
    const seen = await recorded();
    assert.equal(seen[0].body, '{"name":"N"}');
    assert.equal(seen[1].body, '{"email":null}');
    assert.equal(seen[2].body, '{"name":"N"}');
    assert.deepEqual(seen[3].query, {});
  });
});

describe('forward compatibility', () => {
  it('accepts enum values the spec does not list', async () => {
    const invoice = await client().invoices.get('inv_5');
    assert.equal(invoice.status, 'uncollectible');
    assert.equal(invoice.reminder_level, 3);
  });

  it('keeps unknown object fields', async () => {
    const fake = async () => json({ id: 'cus_1', name: 'N', created_at: 'x', brand_new_field: { a: 1 } });
    const customer = await client({ fetch: fake }).customers.get('cus_1');
    assert.deepEqual(customer.brand_new_field, { a: 1 });
  });
});

describe('pagination', () => {
  it('walks cursor pages with for await, without awaiting the call first', async () => {
    const ids = [];
    for await (const c of client().customers.list({ limit: 2 })) ids.push(c.id);
    assert.deepEqual(ids, ['cus_1', 'cus_2', 'cus_3', 'cus_4', 'cus_5']);
    const seen = (await recorded()).map((r) => r.query.starting_after);
    assert.deepEqual(seen, [undefined, 'cus_2', 'cus_4']);
  });

  it('walks offset pages', async () => {
    const ids = [];
    for await (const i of client().invoices.list({ limit: 2 })) ids.push(i.id);
    assert.deepEqual(ids, ['inv_1', 'inv_2', 'inv_3', 'inv_4', 'inv_5']);
    assert.deepEqual((await recorded()).map((r) => r.query.offset), [undefined, '2', '4', '5']);
  });

  it('walks page-number pages', async () => {
    const ids = [];
    for await (const p of await client().payments.list()) ids.push(p.id);
    assert.equal(ids.length, 6);
    assert.deepEqual((await recorded()).map((r) => r.query.page), [undefined, '2', '3', '4']);
  });

  it('exposes items, hasNextPage and getNextPage', async () => {
    const page = await client().customers.list({ limit: 2 });
    assert.deepEqual(page.items.map((c) => c.id), ['cus_1', 'cus_2']);
    assert.equal(page.response.next_cursor, 'cus_2');
    assert.equal(page.hasNextPage(), true);
    const second = await page.getNextPage();
    assert.deepEqual(second.items.map((c) => c.id), ['cus_3', 'cus_4']);
    const last = await second.getNextPage();
    assert.deepEqual(last.items.map((c) => c.id), ['cus_5']);
    assert.equal(last.hasNextPage(), false);
    await assert.rejects(last.getNextPage(), sdk.LedgerError);
  });

  it('keeps per-request options on later pages', async () => {
    for await (const _ of client().customers.list({ limit: 2 }, { headers: { 'x-custom': 'yes' } }));
    assert.ok((await recorded()).every((r) => r.headers['x-custom'] === 'yes'));
  });
});

describe('streaming', () => {
  it('yields typed events, including types the spec does not define', async () => {
    const stream = await client().events.stream();
    const events = [];
    for await (const event of stream) events.push(event);
    assert.deepEqual(events.map((e) => e.type), ['invoice.paid', 'payment.failed', 'customer.created']);
    assert.equal(events[0].invoice.id, 'inv_3');
    assert.equal(events[1].payment.failure_reason, 'card_declined');
    assert.equal((await recorded())[0].headers.accept, 'text/event-stream');
  });

  it('closes the connection on break', async () => {
    const stream = await client().events.stream();
    for await (const event of stream) {
      assert.equal(event.type, 'invoice.paid');
      break;
    }
    assert.equal(stream.controller.signal.aborted, true);
  });

  it('ends quietly on controller.abort()', async () => {
    const stream = await client().events.stream();
    stream.controller.abort();
    const events = [];
    for await (const event of stream) events.push(event);
    assert.ok(events.length < 3);
  });

  it('retries the connection like any request', async () => {
    const stream = await client().events.stream(fail('503:1', 's1'));
    let count = 0;
    for await (const _ of stream) count++;
    assert.equal(count, 3);
    assert.equal((await recorded()).length, 2);
  });
});

describe('files', () => {
  const text = 'receipt-bytes-123';
  const bytes = new TextEncoder().encode(text);
  const variants = {
    Blob: () => new Blob([bytes], { type: 'text/plain' }),
    Uint8Array: () => bytes,
    ArrayBuffer: () => bytes.buffer.slice(0),
    ReadableStream: () => new Response(bytes).body,
    Response: () => new Response(bytes),
  };

  for (const [kind, make] of Object.entries(variants)) {
    it(`uploads a ${kind} as multipart`, async () => {
      const file = await client().files.upload({ file: make(), purpose: 'receipt' });
      assert.equal(file.id, 'file_1');
      const [request] = await recorded();
      assert.match(request.headers['content-type'], /^multipart\/form-data; boundary=/);
      assert.ok(request.body.includes(text));
      assert.ok(request.body.includes('name="purpose"') && request.body.includes('receipt'));
    });
  }

  it('keeps the filename of a File made with toFile', async () => {
    const file = await sdk.toFile(bytes, 'receipt.txt', { type: 'text/plain' });
    const stored = await client().files.upload({ file });
    assert.equal(stored.filename, 'receipt.txt');
    assert.equal(stored.size_bytes > bytes.length, true);
  });

  for (const [kind, make] of Object.entries(variants)) {
    it(`uploads a ${kind} as a raw body`, async () => {
      const file = await client().files.uploadRaw({ filename: 'a.bin', body: make() });
      assert.equal(file.size_bytes, bytes.length);
      assert.equal(file.filename, 'a.bin');
      const [request] = await recorded();
      assert.equal(request.body, text);
      assert.equal(request.query.filename, 'a.bin');
    });
  }

  it('downloads a PDF as a Response', async () => {
    const response = await client().invoices.downloadPdf('inv_1');
    assert.equal(response.headers.get('content-type'), 'application/pdf');
    const buffer = await response.arrayBuffer();
    assert.equal(new TextDecoder().decode(buffer.slice(0, 5)), '%PDF-');
  });

  it('resolves to undefined for an empty response', async () => {
    assert.equal(await client().customers.delete('cus_1'), undefined);
  });
});

describe('webhooks', () => {
  const secretBytes = Buffer.from('webhook-signing-key-0123456789');
  const secret = `whsec_${secretBytes.toString('base64')}`;
  const body = JSON.stringify({ type: 'invoice.paid', invoice: { id: 'inv_3' } });
  const sign = (id, timestamp, payload = body) => `v1,${createHmac('sha256', secretBytes).update(`${id}.${timestamp}.${payload}`).digest('base64')}`;
  const now = () => Math.floor(Date.now() / 1000);
  const headers = (extra = {}, timestamp = now()) => ({ 'webhook-id': 'msg_1', 'webhook-timestamp': String(timestamp), 'webhook-signature': sign('msg_1', timestamp), ...extra });
  const c = () => client({ webhookSecret: secret });

  it('returns the typed event for a valid signature', async () => {
    const event = await c().webhooks.unwrap(body, headers());
    assert.equal(event.type, 'invoice.paid');
    assert.equal(event.invoice.id, 'inv_3');
  });

  it('accepts a Headers object, mixed-case names, and bytes', async () => {
    assert.equal((await c().webhooks.unwrap(body, new Headers(headers()))).type, 'invoice.paid');
    const mixed = Object.fromEntries(Object.entries(headers()).map(([k, v]) => [k.replace(/^w/, 'W'), v]));
    assert.equal((await c().webhooks.unwrap(body, mixed)).type, 'invoice.paid');
    assert.equal((await c().webhooks.unwrap(new TextEncoder().encode(body), headers())).type, 'invoice.paid');
  });

  it('accepts any matching signature among several', async () => {
    const ts = now();
    const header = headers({ 'webhook-signature': `v1,AAAA ${sign('msg_1', ts)} v2,zzzz` }, ts);
    assert.equal((await c().webhooks.unwrap(body, header)).type, 'invoice.paid');
  });

  it('reads the secret from LEDGER_WEBHOOK_SECRET', async () => {
    process.env.LEDGER_WEBHOOK_SECRET = secret;
    assert.equal((await new Ledger({ baseURL: base }).webhooks.unwrap(body, headers())).type, 'invoice.paid');
  });

  it('rejects a bad signature, a tampered body, old and future timestamps, and missing headers', async () => {
    const rejects = (promise, pattern) => assert.rejects(promise, (e) => e instanceof sdk.WebhookVerificationError && pattern.test(e.message));
    await rejects(c().webhooks.unwrap(body, headers({ 'webhook-signature': 'v1,AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=' })), /signature/);
    await rejects(c().webhooks.unwrap(body.replace('inv_3', 'inv_9'), headers()), /signature/);
    const old = now() - 301;
    await rejects(c().webhooks.unwrap(body, headers({ 'webhook-signature': sign('msg_1', old) }, old)), /old/);
    const future = now() + 301;
    await rejects(c().webhooks.unwrap(body, headers({ 'webhook-signature': sign('msg_1', future) }, future)), /future/);
    const slightlyOld = now() - 290;
    await c().webhooks.unwrap(body, headers({ 'webhook-signature': sign('msg_1', slightlyOld) }, slightlyOld));
    await rejects(c().webhooks.unwrap(body, { 'webhook-id': 'msg_1' }), /Missing/);
    await rejects(c().webhooks.unwrap(body, headers({ 'webhook-signature': sign('msg_1', now()).replace('v1', 'v2') })), /signature/);
  });

  it('refuses to verify without a secret', async () => {
    await assert.rejects(new Ledger({ baseURL: base }).webhooks.unwrap(body, headers()), /LEDGER_WEBHOOK_SECRET/);
  });
});

describe('logging and deprecation', () => {
  it('logs requests at debug level with credentials redacted', async () => {
    process.env.LEDGER_LOG = 'debug';
    const lines = [];
    const logger = { debug: (...a) => lines.push(a), info: (...a) => lines.push(a), warn: (...a) => lines.push(a), error: (...a) => lines.push(a) };
    const c = new Ledger({ baseURL: base, clientId: 'test-client', clientSecret: 'test-secret', tokenURL: `${base}/oauth/token`, logger });
    await c.customers.get('cus_1');
    await new Ledger({ baseURL: base, apiKey: 'test-key', logger }).customers.get('cus_1');
    const text = JSON.stringify(lines);
    assert.match(text, /GET/);
    assert.match(text, /\/customers\/cus_1/);
    assert.match(text, /\*\*\*/);
    for (const secret of ['test-key', 'test-secret', 'tok_1']) assert.ok(!text.includes(secret), `${secret} leaked into the log`);
  });

  it('is quiet by default', async () => {
    const lines = [];
    const logger = { debug: (...a) => lines.push(a), info: (...a) => lines.push(a), warn: (...a) => lines.push(a), error: (...a) => lines.push(a) };
    await client({ logger }).customers.get('cus_1');
    assert.deepEqual(lines, []);
  });

  it('warns once about a deprecated method and still works', async () => {
    const warnings = [];
    const original = console.warn;
    console.warn = (...a) => warnings.push(a.join(' '));
    try {
      const first = await client().customers.search({ q: 'customer 1' });
      await client().customers.search({ q: 'customer 2' });
      assert.equal(first[0].id, 'cus_1');
    } finally {
      console.warn = original;
    }
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /customers\.search/);
    assert.match(warnings[0], /customers\.list/);
  });
});
