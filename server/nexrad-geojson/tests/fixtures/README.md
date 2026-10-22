# Fixtures

`ktlx-mini.ar2` is a trimmed real NEXRAD Archive II volume used by the parser
and golden-output tests. It was derived from:

```text
https://unidata-nexrad-level2.s3.amazonaws.com/2026/10/20/KTLX/KTLX20261020_000251_V06
```

The file is an Archive II volume (version 06, KTLX) of 2,870,734 bytes.

## Extraction rule

Keep, in this order:

1. the original 24-byte volume header;
2. the original compressed metadata LDM record, unchanged;
3. a new bzip2 LDM record built from the first 20 Message 31 radials of the
   original first radial LDM record.

Each radial occupies a fixed 9,956-byte slot in the original record
(`RadialLength == 9928` at byte 18 of the Message 31 header).

```python
import bz2, pathlib, struct

src = pathlib.Path('/tmp/KTLX20261020_000251_V06').read_bytes()
header = src[:24]
pos = 24
meta_size = struct.unpack('>i', src[pos:pos+4])[0]
pos += 4
meta = src[pos:pos+meta_size]
pos += meta_size
rad_size = struct.unpack('>i', src[pos:pos+4])[0]
pos += 4
radials = bz2.decompress(src[pos:pos+rad_size])
assert len(radials) == 120 * 9956, len(radials)
assert struct.unpack('>H', radials[28+18:28+20])[0] == 9928
payload = radials[:20 * 9956]
comp = bz2.compress(payload, 9)
out = header + struct.pack('>i', meta_size) + meta + struct.pack('>i', len(comp)) + comp
dest = pathlib.Path('server/nexrad-geojson/tests/fixtures/ktlx-mini.ar2')
dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_bytes(out)
```

The result is 18,972 bytes: 24 volume header + 4 size + 2,348 metadata record +
4 size + 16,592 compressed radials.

## Expected output

`ktlx-mini-REF-1.expected.json` was generated with the upstream Go tool at
commit `53e2aa5fd7a5ffba06ad4e3089f001b46b646a67`:

```sh
git clone https://github.com/jtleniger/go-nexrad-geojson /tmp/go-nexrad-ref
cd /tmp/go-nexrad-ref
git checkout 53e2aa5fd7a5ffba06ad4e3089f001b46b646a67
go build -o /tmp/go-nexrad-ref-bin .
/tmp/go-nexrad-ref-bin --minimum 0 -e 1 -p REF -o /tmp/golden \
  server/nexrad-geojson/tests/fixtures/ktlx-mini.ar2
cp /tmp/golden-REF-1.json \
  server/nexrad-geojson/tests/fixtures/ktlx-mini-REF-1.expected.json
```

It contains 274 features. `--minimum 0` exercises the optional minimum filter
while keeping enough gates for a meaningful comparison.
