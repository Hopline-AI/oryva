// Type-level checks for scripts/smoke-typescript.sh: what an SDK user's editor and tsc see.
// Compiled with `tsc --noEmit`; it is never run. Each `@ts-expect-error` fails the build if the
// line below it stops being an error.

import Ledger, { type Invoice, type Page } from '../src/index.js';

const client = new Ledger({ apiKey: 'key' });

export async function usage() {
  const customer = await client.customers.create({ name: 'Ada', email: null });
  const id: string = customer.id;

  // @ts-expect-error `name` is required
  await client.customers.create({ email: 'ada@example.com' });
  // @ts-expect-error `name` is a string
  await client.customers.create({ name: 5 });
  // @ts-expect-error there is no such method
  client.customers.nope();
  // @ts-expect-error `timeout` is a number
  await client.customers.get(id, { timeout: '5' });

  // Discriminated union: narrowing on `type` gives the right payload.
  for await (const event of await client.events.stream()) {
    switch (event.type) {
      case 'invoice.paid': {
        const invoice: Invoice = event.invoice;
        break;
      }
      case 'payment.failed': {
        const reason: string | null | undefined = event.payment.failure_reason;
        break;
      }
    }
  }

  // Lists iterate to items; the awaited page exposes its items and the full response.
  for await (const c of client.customers.list()) {
    const name: string = c.name;
  }
  const page: Page<{ id: string }> = await client.customers.list({ limit: 2 });
  const next: string | null | undefined = (await client.customers.list()).response.next_cursor;

  // Enums accept values the spec doesn't list.
  const invoice = await client.invoices.get('inv_1');
  const status: 'draft' | 'open' | 'paid' | 'void' | (string & {}) = invoice.status;
  const level: 0 | 1 | 2 | (number & {}) | undefined = invoice.reminder_level;
  await client.invoices.list({ status: 'something_new' });

  const { data, response } = await client.customers.get(id).withResponse();
  const code: number = response.status;
  const pdf: Response = await client.invoices.downloadPdf('inv_1');
  await client.files.uploadRaw({ filename: 'a.bin', body: new Uint8Array(4) });

  const hook = await client.webhooks.unwrap('{}', {});
  if (hook.type === 'payment.failed') {
    const failed: string = hook.payment.id;
  }
}
