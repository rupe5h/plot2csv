// Help menu checks: shortcuts and licence dialogs, and "Download for Offline Use".
// The downloaded copy is opened from disk and must boot and run OCR on its own.
//   node tools/e2e/help.mjs dist/plot2csv.html tests/fixtures/sine_basic.png target/e2e
import { launch, sleep, b64 } from "./cdp.mjs";
import fs from "node:fs";
import path from "node:path";
const [html, img, out] = process.argv.slice(2);
const dl = path.resolve(out, "offline");
fs.rmSync(dl, { recursive: true, force: true });
fs.mkdirSync(dl, { recursive: true });
const b = await launch({ width: 1400, height: 900 });
const esc = () => b.key("Escape", "Escape");
const booted = "!!globalThis.wpApp && !document.getElementById('wp_loading')";
// Help menu and its items at 1400x900.
const help = async () => { await b.click(288, 12); await sleep(300); };
const item = async (y) => { await b.click(312, y); await sleep(400); };
const [SHORTCUTS, DOWNLOAD, LICENCES] = [61, 92, 115];
try {
  await b.send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: dl });
  await b.goto("file://" + path.resolve(html));
  await b.waitFor(booted, 60000);
  await b.click(729, 317); await sleep(200); // dismiss the Import tip
  await help(); await b.screenshot(`${out}/help_menu.png`);
  await item(SHORTCUTS); await b.screenshot(`${out}/help_shortcuts.png`); await esc(); await sleep(200);
  await help(); await item(LICENCES); await b.screenshot(`${out}/help_licences.png`); await esc(); await sleep(200);

  await help(); await item(DOWNLOAD);
  const file = path.join(dl, "plot2csv.html");
  for (let i = 0; i < 100 && !(fs.existsSync(file) && fs.statSync(file).size > 0); i++) await sleep(200);
  if (!fs.existsSync(file)) throw new Error("offline copy was not downloaded");
  await sleep(1000); // let the download finish writing
  const size = fs.statSync(file).size;
  const orig = fs.statSync(html).size;
  console.log(`offline copy: ${size} B (original ${orig} B)`);
  if (Math.abs(size - orig) > orig * 0.01) throw new Error("offline copy size differs from the original by more than 1%");
  await b.screenshot(`${out}/help_downloaded.png`);

  // The saved copy must work on its own: boot, run OCR, and be able to save itself again.
  await b.goto("file://" + file);
  await b.waitFor(booted, 60000);
  if (!(await b.eval("typeof globalThis.wpSaveOffline === 'function'"))) throw new Error("saved copy cannot save itself");
  const lines = JSON.parse(await b.eval(`(async () => {
    const bytes = Uint8Array.from(atob(${JSON.stringify(b64(img))}), c => c.charCodeAt(0));
    return await wpApp.ocr_image_json(bytes);
  })()`));
  console.log(`saved copy OCR: ${lines.length} lines`);
  if (!lines.length) throw new Error("OCR found no text in the saved copy");
  await b.screenshot(`${out}/help_offline_copy.png`);
} catch (e) { console.error("FAILED:", e.message); process.exitCode = 1; }
finally { console.log(b.logs.filter((l) => l.includes("error") || l.includes("exception") || l.includes("panic")).slice(-10).join("\n")); await b.close(); }
