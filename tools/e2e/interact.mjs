// Interactive checks: dialogs, manual data tools, measurement, export preview.
import { launch, sleep, b64 } from "./cdp.mjs";
import path from "node:path";
const [html, img, out] = process.argv.slice(2);
const b = await launch({ width: 1400, height: 900 });
const key = async (k, code = k, vk = 0, text) => {
  await b.send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk, text });
  await b.send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk });
};
const enter = () => key("Enter", "Enter", 13, "\r");
// sine_basic at 1400x900: image px -> screen
const S = (x, y) => [18 + 1.28 * x, 110 + 1.28 * y];
const clickImg = async (x, y) => { const [sx, sy] = S(x, y); await b.click(sx, sy); await sleep(150); };
try {
  await b.goto("file://" + path.resolve(html));
  await b.waitFor("!!globalThis.wpApp && !document.getElementById('wp_loading')", 60000);
  await b.eval(`globalThis.wpPasted.push(Uint8Array.from(atob(${JSON.stringify(b64(img))}), c => c.charCodeAt(0)))`);
  await sleep(1200);
  for (const w of [2500, 2500, 2500, 4500]) { await b.click(600, 450); await enter(); await sleep(w); }
  // Axes stage: open the plot type dialog via the Axes menu.
  await b.click(54, 12); await sleep(300); await b.screenshot(`${out}/a_axes_menu.png`);
  await key("Escape", "Escape", 27); await sleep(300);
  await b.click(600, 450); await enter(); await sleep(3000); // -> Data
  // Manual mode: A, then add three points on the orange curve.
  await key("a", "KeyA", 65, "a"); await sleep(200);
  for (const [x, y] of [[300, 82], [400, 140], [500, 270]]) await clickImg(x, y);
  await key("s", "KeyS", 83, "s"); await sleep(200);
  await clickImg(400, 140);
  for (let i = 0; i < 5; i++) await key("ArrowUp", "ArrowUp", 38);
  await sleep(400);
  await b.screenshot(`${out}/b_manual.png`);
  // Data -> View Data dialog
  await b.click(91, 12); await sleep(300);
  await b.screenshot(`${out}/c_data_menu.png`);
  await key("Escape", "Escape", 27); await sleep(200);
  // Analyze -> Measure Distances
  await b.click(137, 12); await sleep(300); await b.click(170, 37); await sleep(300);
  await clickImg(90, 480); await clickImg(760, 480); await sleep(300);
  await b.screenshot(`${out}/d_measure.png`);
  // Export stage + plot preview
  await b.click(600, 450); await enter(); await sleep(2000);
  await b.screenshot(`${out}/e_export.png`);
} catch (e) { console.error("FAILED:", e.message); process.exitCode = 1; }
finally { console.log(b.logs.filter((l) => l.includes("error") || l.includes("exception") || l.includes("panic")).slice(-10).join("\n")); await b.close(); }
