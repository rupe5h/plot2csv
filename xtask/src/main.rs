//! Build tasks.
//!
//!   cargo xtask fetch          download onnxruntime-web + indra models into vendor/
//!   cargo xtask dist [--dev] [--wasm-opt]   build dist/plot2csv.html (single self-contained file)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use base64::Engine;
use sha2::Digest;

const ORT_VERSION: &str = "1.31.1";
const INDRA_REV: &str = "v1.1.0";
const MODELS: &[(&str, &str)] = &[
    ("det.onnx", "f2bd4cba23e9c328cf7ec824634e423e191a2c6aacbbf35c62bd79dd70f62f59"),
    ("rec.onnx", "475972a2710be4a3cbfeb2bc7f3624ffd563de07de42476fc7a290d4e1ece484"),
    ("meta.json", "4ce4c94b46f5d644033c02e718322014ff4081270f385038c571912b56696339"),
    ("LICENSE", "50c88a018a7ee322e89f6b09401b237beb0cd331685879405b3db48993950a5f"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("$ {:?}", cmd);
    let st = cmd.status().with_context(|| format!("running {:?}", cmd))?;
    if !st.success() {
        bail!("command failed: {:?}", cmd);
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn fetch() -> Result<()> {
    let vendor = root().join("vendor");
    let ort_dir = vendor.join("ort");
    std::fs::create_dir_all(&ort_dir)?;
    if !ort_dir.join("package/dist/ort.wasm.bundle.min.mjs").exists() {
        run(Command::new("npm").args(["pack", &format!("onnxruntime-web@{ORT_VERSION}")]).current_dir(&ort_dir))?;
        run(Command::new("tar").args(["-xzf", &format!("onnxruntime-web-{ORT_VERSION}.tgz")]).current_dir(&ort_dir))?;
    }
    let mdir = vendor.join("models");
    std::fs::create_dir_all(&mdir)?;
    for (name, hash) in MODELS {
        let p = mdir.join(name);
        if !p.exists() {
            let url = format!("https://huggingface.co/datalab-to/indra-ocr/resolve/{INDRA_REV}/{name}");
            run(Command::new("curl").args(["-sSfL", "-o"]).arg(&p).arg(&url))?;
        }
        let got = sha256(&std::fs::read(&p)?);
        if got != *hash {
            bail!("{} sha256 mismatch: {got}", p.display());
        }
    }
    eprintln!("vendor/ ready");
    Ok(())
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(bytes)?;
    Ok(e.finish()?)
}

fn payload_tag(id: &str, bytes: &[u8]) -> Result<String> {
    let gz = gzip(bytes)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&gz);
    eprintln!("  payload {id:<10} {:>9} B raw  {:>9} B gz  {:>9} B b64", bytes.len(), gz.len(), b64.len());
    Ok(format!("<script type=\"application/octet-stream\" id=\"payload-{id}\" data-gz=\"1\">{b64}</script>\n"))
}

/// Turn the ES module's trailing `export{a as B,...}` into `globalThis.ort={B:a,...}`
/// so the bundle can be inlined without being importable.
fn ort_module_to_global(src: &str) -> Result<String> {
    let src = src.trim_end();
    let src = src.strip_suffix("//# sourceMappingURL=ort.wasm.bundle.min.mjs.map").unwrap_or(src).trim_end();
    let start = src.rfind("export{").context("ORT bundle: no trailing export")?;
    let end = src[start..].find('}').context("ORT bundle: unterminated export")? + start;
    let mut fields = Vec::new();
    for part in src[start + 7..end].split(',') {
        let part = part.trim();
        let (local, exported) = match part.split_once(" as ") {
            Some((l, e)) => (l.trim(), e.trim()),
            None => (part, part),
        };
        fields.push(format!("{exported}:{local}"));
    }
    Ok(format!("{}globalThis.ort={{{}}};{}", &src[..start], fields.join(","), &src[end + 1..]))
}

/// Keep only the parts of indra's meta.json the app needs (drops the big CJK vocabularies).
fn slim_meta(bytes: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(bytes)?;
    // Minimal JSON surgery without a JSON dependency: rebuild from known keys.
    let get_num = |k: &str| -> Option<String> {
        let i = text.find(&format!("\"{k}\":"))? + k.len() + 3;
        let rest = text[i..].trim_start();
        let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        Some(n)
    };
    let vi = text.find("\"vocab\":").context("meta.json: vocab")? + 8;
    let rest = text[vi..].trim_start();
    // vocab is a JSON string literal; find its end honouring escapes.
    let mut end = 1;
    let b = rest.as_bytes();
    while end < b.len() {
        match b[end] {
            b'\\' => end += 2,
            b'"' => break,
            _ => end += 1,
        }
    }
    let vocab = &rest[..=end];
    let out = format!(
        "{{\"det_long_side\": {}, \"rec_height\": {}, \"rec_window\": {}, \"vocab\": {}}}",
        get_num("det_long_side").unwrap_or("1024".into()),
        get_num("rec_height").unwrap_or("32".into()),
        get_num("rec_window").unwrap_or("1".into()),
        vocab
    );
    Ok(out.into_bytes())
}

fn dist(dev: bool) -> Result<()> {
    let root = root();
    let profile = if dev { "dev" } else { "release" };
    let mut c = Command::new("cargo");
    c.current_dir(&root).args(["build", "-p", "plot2csv-app", "--lib", "--target", "wasm32-unknown-unknown"]);
    if !dev {
        c.arg("--release");
    }
    run(&mut c)?;
    let target_dir = root.join("target/wasm32-unknown-unknown").join(if dev { "debug" } else { "release" });
    let out = root.join("target/wasm-bindgen").join(profile);
    std::fs::create_dir_all(&out)?;
    run(Command::new("wasm-bindgen")
        .arg(target_dir.join("plot2csv_app.wasm"))
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(&out))?;
    let mut wasm_path = out.join("plot2csv_app_bg.wasm");
    // wasm-opt -Oz shrinks the raw module slightly but the gzipped payload grows,
    // so it is opt-in.
    if !dev && std::env::args().any(|a| a == "--wasm-opt") && Command::new("wasm-opt").arg("--version").output().is_ok() {
        let opt = out.join("plot2csv_app_opt.wasm");
        run(Command::new("wasm-opt")
            .arg(&wasm_path)
            .args([
                "-Oz",
                "--enable-bulk-memory",
                "--enable-nontrapping-float-to-int",
                "--enable-sign-ext",
                "--enable-reference-types",
                "--enable-multivalue",
                "--enable-mutable-globals",
                "-o",
            ])
            .arg(&opt))?;
        wasm_path = opt;
    }
    let glue = std::fs::read_to_string(out.join("plot2csv_app.js"))?;
    if !glue.contains("__wbg_init") {
        bail!("wasm-bindgen glue has no __wbg_init");
    }
    let app_wasm = std::fs::read(&wasm_path)?;

    let vendor = root.join("vendor");
    let ort_dist = vendor.join("ort/package/dist");
    if !ort_dist.exists() {
        bail!("vendor/ missing — run `cargo xtask fetch` first");
    }
    let ort_js = ort_module_to_global(&std::fs::read_to_string(ort_dist.join("ort.wasm.bundle.min.mjs"))?)?;
    let ort_wasm = std::fs::read(ort_dist.join("ort-wasm-simd-threaded.wasm"))?;
    let models = vendor.join("models");
    let det = std::fs::read(models.join("det.onnx"))?;
    let rec = std::fs::read(models.join("rec.onnx"))?;
    let meta = slim_meta(&std::fs::read(models.join("meta.json"))?)?;

    let web = root.join("crates/plot2csv-app/web");
    let template = std::fs::read_to_string(web.join("index.template.html"))?;
    let bridge = std::fs::read_to_string(web.join("ort_bridge.js"))?;

    let mut payloads = String::new();
    payloads += &payload_tag("app_wasm", &app_wasm)?;
    payloads += &payload_tag("ort_wasm", &ort_wasm)?;
    payloads += &payload_tag("det", &det)?;
    payloads += &payload_tag("rec", &rec)?;
    payloads += &payload_tag("meta", &meta)?;

    // `</script>` must not appear inside inline scripts.
    let esc = |s: &str| s.replace("</script", "<\\/script");
    let html = template
        .replace("<!--PAYLOADS-->", &payloads)
        .replace("/*ORT_JS*/", &esc(&ort_js))
        .replace("/*BRIDGE_JS*/", &esc(&bridge))
        .replace("/*APP_JS*/", &esc(&glue));
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist)?;
    let path = dist.join("plot2csv.html");
    std::fs::write(&path, html.as_bytes())?;
    eprintln!("wrote {} ({:.1} MB)", path.display(), html.len() as f64 / 1e6);
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("fetch") => fetch(),
        Some("dist") => dist(args.iter().any(|a| a == "--dev")),
        _ => {
            eprintln!("usage: cargo xtask <fetch|dist [--dev]>");
            std::process::exit(2);
        }
    }
}
