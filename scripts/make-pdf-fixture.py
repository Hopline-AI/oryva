#!/usr/bin/env python3
"""Writes fixtures/evals/files/15-pages.pdf: 15 pages, one line of text each. Stdlib only."""
import pathlib

PAGES = 15
objs = ["<< /Type /Catalog /Pages 2 0 R >>", None, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
kids = []
for n in range(1, PAGES + 1):
    page_id = len(objs) + 1
    kids.append(f"{page_id} 0 R")
    stream = f"BT /F1 24 Tf 72 700 Td (Page {n} of {PAGES}) Tj ET"
    objs.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {page_id + 1} 0 R >>")
    objs.append(f"<< /Length {len(stream)} >>\nstream\n{stream}\nendstream")
objs[1] = f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {PAGES} >>"

out = b"%PDF-1.4\n"
offsets = []
for i, body in enumerate(objs, 1):
    offsets.append(len(out))
    out += f"{i} 0 obj\n{body}\nendobj\n".encode()
xref = len(out)
out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offsets)
out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()

dest = pathlib.Path(__file__).resolve().parent.parent / "fixtures/evals/files/15-pages.pdf"
dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_bytes(out)
print(f"wrote {dest} ({len(out)} bytes)")
