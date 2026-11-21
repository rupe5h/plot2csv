// Minimal Chrome DevTools Protocol driver for end-to-end checks (Node >= 22).
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

export async function launch({ width = 1400, height = 900, headless = true } = {}) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "wp-chrome-"));
  const port = 9300 + Math.floor(Math.random() * 500);
  const args = [
    `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-first-run",
    "--no-default-browser-check", `--window-size=${width},${height}`, "about:blank",
  ];
  if (headless) args.unshift("--headless=new", "--use-angle=swiftshader", "--enable-unsafe-swiftshader");
  const proc = spawn(CHROME, args, { stdio: "ignore" });
  let ver;
  for (let i = 0; i < 100; i++) {
    try { ver = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); if (ver.length) break; } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  const page = ver.find((t) => t.type === "page");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  let id = 0;
  const pending = new Map();
  const listeners = [];
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      const { res, rej } = pending.get(m.id); pending.delete(m.id);
      m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result);
    } else if (m.method) listeners.forEach((l) => l(m));
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params }));
  });
  const logs = [];
  listeners.push((m) => {
    if (m.method === "Runtime.consoleAPICalled")
      logs.push(`[${m.params.type}] ` + m.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
    if (m.method === "Runtime.exceptionThrown") logs.push("[exception] " + JSON.stringify(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text));
  });
  await send("Runtime.enable");
  await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  const api = {
    send, logs,
    async goto(url) { await send("Page.navigate", { url }); },
    async eval(expr, { timeout = 120000 } = {}) {
      const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true, timeout });
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
      return r.result.value;
    },
    async waitFor(expr, ms = 60000) {
      const t0 = Date.now();
      while (Date.now() - t0 < ms) {
        try { if (await api.eval(expr)) return true; } catch {}
        await new Promise((r) => setTimeout(r, 200));
      }
      throw new Error("timeout waiting for " + expr);
    },
    async screenshot(file) {
      const r = await send("Page.captureScreenshot", { format: "png" });
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, Buffer.from(r.data, "base64"));
    },
    async mouse(type, x, y, button = "left", clickCount = 1) {
      await send("Input.dispatchMouseEvent", { type, x, y, button, clickCount });
    },
    async click(x, y) { await api.mouse("mouseMoved", x, y); await api.mouse("mousePressed", x, y); await api.mouse("mouseReleased", x, y); },
    async key(key, code = key, text) {
      await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, text });
      await send("Input.dispatchKeyEvent", { type: "keyUp", key, code });
    },
    async close() { try { await send("Browser.close"); } catch {} proc.kill(); await new Promise((r) => setTimeout(r, 300)); try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5 }); } catch {} },
  };
  return api;
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export function b64(file) { return fs.readFileSync(file).toString("base64"); }
