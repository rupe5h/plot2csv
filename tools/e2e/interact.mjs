// Interactive checks: dialogs, manual data tools, measurement, export preview.
import { launch, sleep, b64 } from "./cdp.mjs";
import path from "node:path";
const [html, img, out] = process.argv.slice(2);
const b = await launch({ width: 1400, height: 900 });
const key = async (k, code = k, vk = 0, text, modifiers = 0) => {
  await b.send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk, text, modifiers });
  await b.send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
};
const enter = () => key("Enter", "Enter", 13, "\r");
// Cmd on macOS (the page reports a Mac user agent there), Ctrl elsewhere; Shift = 8.
const CMD = process.platform === "darwin" ? 4 : 2;
const undo = async () => { await key("z", "KeyZ", 90, undefined, CMD); await sleep(300); };
const redo = async () => { await key("Z", "KeyZ", 90, undefined, CMD | 8); await sleep(300); };
// Menu bar at 1400x900 (x centres).
const MENU = { edit: 56, axes: 96, data: 136, analyze: 189 };
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
  await b.click(MENU.axes, 12); await sleep(300); await b.screenshot(`${out}/a_axes_menu.png`);
  await key("Escape", "Escape", 27); await sleep(300);
  await b.click(600, 450); await enter(); await sleep(3000); // -> Data
  // Manual mode: A, then add three points on the orange curve.
  await key("a", "KeyA", 65, "a"); await sleep(200);
  for (const [x, y] of [[300, 82], [400, 140], [500, 270]]) await clickImg(x, y);
  await key("s", "KeyS", 83, "s"); await sleep(200);
  await clickImg(400, 140);
  for (let i = 0; i < 5; i++) { await key("ArrowUp", "ArrowUp", 38); await sleep(80); } // one undo step
  await sleep(400);
  await b.screenshot(`${out}/b_manual.png`);
  // Undo the nudges (one step) and the third point, then redo both.
  await undo(); await undo();
  await b.screenshot(`${out}/b2_undo.png`);
  await redo(); await redo();
  await b.screenshot(`${out}/b3_redo.png`);
  // Edit menu: Undo (the nudges), then redo with the keyboard.
  await b.click(MENU.edit, 12); await sleep(300); await b.screenshot(`${out}/b4_edit_menu.png`);
  await b.click(80, 37); await sleep(400); await b.screenshot(`${out}/b5_menu_undo.png`);
  await redo();
  // Data -> View Data dialog
  await b.click(MENU.data, 12); await sleep(300);
  await b.screenshot(`${out}/c_data_menu.png`);
  await key("Escape", "Escape", 27); await sleep(200);
  // Analyze -> Measure Distances
  await b.click(MENU.analyze, 12); await sleep(300); await b.click(212, 37); await sleep(300);
  await clickImg(90, 480); await clickImg(760, 480); await sleep(300);
  await b.screenshot(`${out}/d_measure.png`);
  // Export stage + plot preview
  await b.click(600, 450); await enter(); await sleep(2000);
  await b.screenshot(`${out}/e_export.png`);
} catch (e) { console.error("FAILED:", e.message); process.exitCode = 1; }
finally { console.log(b.logs.filter((l) => l.includes("error") || l.includes("exception") || l.includes("panic")).slice(-10).join("\n")); await b.close(); }
