// Milestone 0 check: open dist/plot2csv.html from file://, run OCR through the Rust pipeline.
import { launch, sleep, b64 } from "./cdp.mjs";
import path from "node:path";
const [html, img, shot] = process.argv.slice(2);
const b = await launch();
try {
  await b.goto("file://" + path.resolve(html));
  await b.waitFor("!!globalThis.wpApp && !document.getElementById('wp_loading')", 60000);
  const t0 = Date.now();
  const json = await b.eval(`(async () => {
    const bytes = Uint8Array.from(atob(${JSON.stringify(b64(img))}), c => c.charCodeAt(0));
    return await wpApp.ocr_image_json(bytes);
  })()`);
  const lines = JSON.parse(json);
  console.log(`OCR ${lines.length} lines in ${Date.now() - t0} ms`);
  for (const l of lines) console.log(l.bbox.map((v) => v.toFixed(0)).join(","), l.conf.toFixed(2), JSON.stringify(l.text));
  // paste the image into the app and screenshot
  await b.eval(`globalThis.wpPasted.push(Uint8Array.from(atob(${JSON.stringify(b64(img))}), c => c.charCodeAt(0)))`);
  await sleep(1500);
  if (shot) await b.screenshot(shot);
} catch (e) {
  console.error("FAILED:", e.message);
  process.exitCode = 1;
} finally {
  console.log(b.logs.filter((l) => !l.startsWith("[log] OCR")).slice(-15).join("\n"));
  await b.close();
}
