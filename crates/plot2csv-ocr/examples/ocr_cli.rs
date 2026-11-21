//! cargo run -p plot2csv-ocr --features native-ocr --example ocr_cli -- <models_dir> <image>
use plot2csv_ocr::backend::native::NativeOrt;
use plot2csv_ocr::indra::{ocr_page, IndraConfig};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&args[1]);
    let cfg = IndraConfig::from_meta_json(&std::fs::read_to_string(dir.join("meta.json"))?)?;
    let be = NativeOrt::new(&std::fs::read(dir.join("det.onnx"))?, &std::fs::read(dir.join("rec.onnx"))?)?;
    let img = image::open(&args[2])?.to_rgba8();
    let gray = plot2csv_vision::raster::rgba_to_gray(&img);
    let t = std::time::Instant::now();
    let lines = pollster_block_on(ocr_page(&be, &cfg, &gray))?;
    eprintln!("{} lines in {:?}", lines.len(), t.elapsed());
    for l in lines {
        println!("{:>7.1} {:>7.1} {:>7.1} {:>7.1}  conf {:.2}  {:?}", l.bbox[0], l.bbox[1], l.bbox[2], l.bbox[3], l.conf, l.text);
    }
    Ok(())
}

fn pollster_block_on<F: std::future::Future>(f: F) -> F::Output {
    // The native backend resolves immediately, so a trivial executor is enough.
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker { RawWaker::new(p, &VT) }
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = Box::pin(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) { return v; }
    }
}
