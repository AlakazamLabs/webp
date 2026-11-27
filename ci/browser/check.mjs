// Decodes every file of the oracle corpus (MEDIA_DIR, with manifest.tsv
// and the dwebp/anim_dump outputs) in headless Chromium, through WebCodecs'
// ImageDecoder, and compares the pixels with libwebp's. Prints one line per
// file and exits non-zero if any file failed to decode or differs.
import { chromium } from 'playwright';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const dir = path.resolve(process.env.MEDIA_DIR || 'files');
const page = fs.readFileSync('page.html');
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  if (url.pathname === '/') {
    res.writeHead(200, { 'content-type': 'text/html' });
    return res.end(page);
  }
  const file = path.join(dir, path.basename(decodeURIComponent(url.pathname)));
  if (!file.endsWith('.webp') || !fs.existsSync(file)) {
    res.writeHead(404);
    return res.end();
  }
  res.writeHead(200, { 'content-type': 'image/webp' });
  fs.createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}/`;

function readPam(p) {
  const d = fs.readFileSync(p);
  const end = d.indexOf('ENDHDR\n') + 7;
  const head = d.subarray(0, end).toString();
  const get = (k) => Number(new RegExp(k + ' (\\d+)').exec(head)[1]);
  const w = get('WIDTH'), h = get('HEIGHT'), depth = get('DEPTH');
  let px = d.subarray(end);
  if (depth === 3) {
    const out = Buffer.alloc(w * h * 4);
    for (let i = 0; i < w * h; i++) { px.copy(out, i * 4, i * 3, i * 3 + 3); out[i * 4 + 3] = 255; }
    px = out;
  }
  return { w, h, px };
}

// Opaque pixels must match exactly; translucent ones within the rounding of
// an 8-bit premultiply (255 / alpha), which some decode paths apply.
function compare(a, b) {
  let bad = 0, worst = 0;
  for (let i = 0; i < a.length; i += 4) {
    const al = a[i + 3];
    if (al !== b[i + 3]) { bad++; worst = Math.max(worst, Math.abs(al - b[i + 3])); continue; }
    if (al === 0) continue;
    const tol = al === 255 ? 0 : Math.ceil(255 / al);
    for (let c = 0; c < 3; c++) {
      const d = Math.abs(a[i + c] - b[i + c]);
      if (d > tol) { bad++; worst = Math.max(worst, d); break; }
    }
  }
  return { bad, worst };
}

const manifest = fs.readFileSync(path.join(dir, 'manifest.tsv'), 'utf8').trim().split('\n').map((l) => l.split('\t'));
const browser = await chromium.launch();
console.log(`== chromium ${browser.version()}`);
let passed = 0, failed = 0;
const stats = {};
const p = await browser.newPage();
for (const [name, kind, w, h, frames] of manifest) {
  let r;
  try {
    await p.goto(base + '?f=' + encodeURIComponent(name + '.webp'));
    await p.waitForFunction(() => window.result, null, { timeout: 30000 });
    r = await p.evaluate(() => window.result);
  } catch (e) {
    r = { ok: false, error: String(e).split('\n')[0] };
  }
  const problems = [];
  if (!r.ok) problems.push(`decode: ${r.error}`);
  else {
    const want = kind === 'still' ? 1 : Number(frames);
    if (r.frameCount !== want) problems.push(`${r.frameCount} frames, want ${want}`);
    r.frames.forEach((fr, i) => {
      if (fr.w !== Number(w) || fr.h !== Number(h)) problems.push(`frame ${i}: ${fr.w}x${fr.h}`);
      const got = Buffer.from(fr.data, 'base64');
      if (fr.format === 'I420' || fr.format === 'I420A') {
        // Compare the planes with dwebp's 4:2:0 output (alpha appended).
        const ref = fs.readFileSync(path.join(dir, `${name}.dwebp.yuv`));
        const W = Number(w), H = Number(h), cw = (W + 1) >> 1, ch = (H + 1) >> 1;
        const planes = [[W, H], [cw, ch], [cw, ch]];
        if (fr.format === 'I420A') planes.push([W, H]);
        let off = 0, bad = 0, worst = 0;
        planes.forEach(([pw, ph], k) => {
          const { offset, stride } = fr.layout[k];
          for (let y = 0; y < ph; y++) for (let x = 0; x < pw; x++) {
            const a = got[offset + y * stride + x], b = ref[off + y * pw + x];
            if (a !== b) { bad++; worst = Math.max(worst, Math.abs(a - b)); }
          }
          off += pw * ph;
        });
        if (off !== ref.length) problems.push(`frame ${i}: ${fr.format} has ${off} samples, dwebp ${ref.length}`);
        if (bad) problems.push(`frame ${i}: ${bad} ${fr.format} samples differ from dwebp, worst ${worst}`);
        stats[fr.format] = (stats[fr.format] || 0) + 1;
        return;
      }
      // An RGB format: compare with libwebp's RGBA.
      const { offset, stride } = fr.layout[0];
      const W = Number(w), H = Number(h);
      const rgba = Buffer.alloc(W * H * 4);
      const bgr = fr.format.startsWith('BGR');
      for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) {
        const o = offset + y * stride + x * 4, t = (y * W + x) * 4;
        rgba[t] = got[o + (bgr ? 2 : 0)]; rgba[t + 1] = got[o + 1]; rgba[t + 2] = got[o + (bgr ? 0 : 2)];
        rgba[t + 3] = fr.format.endsWith('X') ? 255 : got[o + 3];
      }
      const ref = kind === 'still'
        ? path.join(dir, `${name}.dwebp.pam`)
        : path.join(dir, `${name}.anim`, `dump_${String(i).padStart(4, '0')}.pam`);
      const { px } = readPam(ref);
      const { bad, worst } = compare(rgba, px);
      if (bad) problems.push(`frame ${i}: ${bad} ${fr.format} pixels differ from libwebp, worst ${worst}`);
      stats[fr.format] = (stats[fr.format] || 0) + 1;
    });
  }
  problems.length ? failed++ : passed++;
  console.log(`${problems.length ? 'FAIL' : 'PASS'} ${name}${problems.length ? ': ' + problems.join('; ') : ''}`);
}
await browser.close();
server.close();
console.log(`chromium: ${passed} passed, ${failed} failed; frame formats ${JSON.stringify(stats)}`);
process.exit(failed ? 1 : 0);
