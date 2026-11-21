// Walk the whole pipeline in headless Chrome: paste an image, approve every stage
// with Enter, screenshot each stage. Usage: node pipeline.mjs dist/plot2csv.html image.png outdir
import { launch, sleep, b64 } from "./cdp.mjs";
import path from "node:path";
const [html, img, out] = process.argv.slice(2);
const b = await launch({ width: 1400, height: 900 });
const enter = async () => {
  await b.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
  await b.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
};
try {
  await b.goto("file://" + path.resolve(html));
  await b.waitFor("!!globalThis.wpApp && !document.getElementById('wp_loading')", 60000);
  await b.eval(`globalThis.wpPasted.push(Uint8Array.from(atob(${JSON.stringify(b64(img))}), c => c.charCodeAt(0)))`);
  await sleep(1200);
  await b.mouse("mouseMoved", 500, 450);
  await b.screenshot(`${out}/1_import.png`);
  const stages = ["2_correct", "3_region", "4_text", "5_axes", "6_data", "7_export"];
  for (const s of stages) {
    await b.click(600, 450); // focus canvas
    await enter();
    await sleep(s === "4_text" ? 4000 : 2500);
    await b.mouse("mouseMoved", 520, 400);
    await sleep(300);
    await b.screenshot(`${out}/${s}.png`);
  }
} catch (e) {
  console.error("FAILED:", e.message);
  process.exitCode = 1;
} finally {
  console.log(b.logs.filter((l) => !l.includes("OCR ")).slice(-20).join("\n"));
  await b.close();
}
